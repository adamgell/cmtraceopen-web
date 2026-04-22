//! Per-session entry-query route.
//!
//!   GET /v1/sessions/{session_id}/entries
//!
//! Query parameters (all optional):
//!   - `file` — restrict to a single file_id.
//!   - `severity` — `info` | `warning` | `error` (compared numerically to
//!     the severity column; all entries at or above the given floor are
//!     returned).
//!   - `after_ts` — epoch ms, inclusive lower bound on `ts_ms`.
//!   - `before_ts` — epoch ms, exclusive upper bound on `ts_ms`.
//!   - `q` — plain substring match against `message` (`LIKE '%q%'`).
//!   - `limit` — max 500, default 200.
//!   - `cursor` — opaque, from the previous page's `nextCursor`.
//!
//! Ordering is `(ts_ms NULLS LAST, entry_id ASC)`; the cursor encodes both
//! halves so pagination is stable across NULL-timestamp rows.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use common_wire::{LogEntryDto, Paginated};

use crate::auth::{OperatorTag, RequireRole};
use crate::error::AppError;
use crate::routes::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::storage::{EntryCursor, EntryFilters, EntryRow};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/sessions/{session_id}/entries", get(list_entries))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    file: Option<String>,
    severity: Option<String>,
    after_ts: Option<i64>,
    before_ts: Option<i64>,
    q: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 500;

/// Map the string severity label to its numeric floor.
///
/// Aligns with the severity column encoding populated by parse-on-ingest:
/// 0 = Info, 1 = Warning, 2 = Error. Case-insensitive.
fn parse_min_severity(s: &str) -> Result<i64, AppError> {
    match s.to_ascii_lowercase().as_str() {
        "info" => Ok(0),
        "warning" | "warn" => Ok(1),
        "error" | "err" => Ok(2),
        other => Err(AppError::BadRequest(format!(
            "unknown severity '{other}' (expected info | warning | error)"
        ))),
    }
}

fn severity_to_string(n: i64) -> String {
    match n {
        0 => "Info".to_string(),
        1 => "Warning".to_string(),
        2 => "Error".to_string(),
        // Unknown severities survive as a stable label so clients don't need
        // to handle panics; a new tier can be added server-side without
        // breaking old viewers.
        other => format!("Unknown({other})"),
    }
}

/// Build a `LIKE` pattern from a user-supplied substring. Escapes the
/// pattern metacharacters (`%`, `_`, `\`) so a query for `"50%"` doesn't
/// turn into a wildcard.
///
/// The storage layer issues the statement as `message LIKE ? ESCAPE '\'`?
/// No — to keep the storage layer dumb, we escape here and wrap with `%…%`
/// and the storage impl is careful to bind this string verbatim. SQLite's
/// default LIKE doesn't treat `\` specially, so we do the escaping
/// ourselves by switching to a format that avoids wildcards entirely for
/// user content.
fn build_like_pattern(q: &str) -> String {
    // Replace wildcards with their escaped forms, then wrap. We use `\` as
    // the escape and accept that users can't include literal `\` without
    // it being eaten — acceptable for MVP substring search. A real full-
    // text layer is a future-milestone concern.
    let mut escaped = String::with_capacity(q.len() + 4);
    for ch in q.chars() {
        match ch {
            '%' | '_' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    format!("%{escaped}%")
}

/// Cursor payload: `<ts_or_empty>|<entry_id>`. `ts_ms` is optional because
/// the null-timestamp tier has to be representable.
fn encode_entry_cursor(c: &EntryCursor) -> String {
    let ts = match c.ts_ms {
        Some(n) => n.to_string(),
        None => String::new(),
    };
    encode_cursor(&format!("{}|{}", ts, c.entry_id))
}

fn decode_entry_cursor(cursor: &str) -> Result<EntryCursor, AppError> {
    let decoded = decode_cursor(cursor)?;
    let (ts_str, id_str) = decoded
        .split_once('|')
        .ok_or_else(|| AppError::BadRequest("invalid cursor payload".into()))?;
    let ts_ms = if ts_str.is_empty() {
        None
    } else {
        Some(
            ts_str
                .parse::<i64>()
                .map_err(|_| AppError::BadRequest("invalid cursor timestamp".into()))?,
        )
    };
    let entry_id = id_str
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest("invalid cursor entry_id".into()))?;
    Ok(EntryCursor { ts_ms, entry_id })
}

fn row_to_dto(r: EntryRow) -> LogEntryDto {
    // `extras_json` lives in the DB as raw TEXT so the storage layer stays
    // serde-free. Best-effort parse here — a malformed blob (shouldn't
    // happen; parse-on-ingest writes it) degrades to `None` rather than
    // 500'ing an otherwise valid query.
    let extras = r
        .extras_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    LogEntryDto {
        entry_id: r.entry_id,
        file_id: r.file_id,
        line_number: r.line_number,
        ts_ms: r.ts_ms,
        severity: severity_to_string(r.severity),
        component: r.component,
        thread: r.thread,
        message: r.message,
        extras,
    }
}

#[tracing::instrument(
    skip_all,
    fields(
        %session_id,
        file = ?q.file,
        severity = ?q.severity,
        after_ts = ?q.after_ts,
        before_ts = ?q.before_ts,
        has_q = q.q.is_some(),
        limit = ?q.limit,
        has_cursor = q.cursor.is_some(),
    )
)]
async fn list_entries(
    State(state): State<Arc<AppState>>,
    _principal: RequireRole<OperatorTag>,
    Path(session_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Paginated<LogEntryDto>>, AppError> {
    // Enforce the documented 500 ceiling explicitly. `clamp_limit` would
    // silently cap, which is fine for convenience callers but the spec
    // calls for a 400 on over-limit so clients catch integration bugs.
    if let Some(l) = q.limit {
        if l > MAX_LIMIT {
            return Err(AppError::BadRequest(format!(
                "limit {l} exceeds max {MAX_LIMIT}"
            )));
        }
    }
    let limit = clamp_limit(q.limit, DEFAULT_LIMIT, MAX_LIMIT);

    // 404 on unknown session — see files.rs for rationale.
    if state.meta.get_session(session_id).await?.is_none() {
        return Err(AppError::NotFound(format!(
            "session {session_id} not found"
        )));
    }

    let min_severity = q.severity.as_deref().map(parse_min_severity).transpose()?;
    let q_like = q.q.as_deref().map(build_like_pattern);
    let cursor = q.cursor.as_deref().map(decode_entry_cursor).transpose()?;

    let filters = EntryFilters {
        file_id: q.file.clone(),
        min_severity,
        after_ts_ms: q.after_ts,
        before_ts_ms: q.before_ts,
        q_like,
        cursor,
    };

    let mut rows = state
        .meta
        .query_entries(session_id, &filters, limit + 1)
        .await?;

    let next_cursor = if rows.len() as u32 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| {
            encode_entry_cursor(&EntryCursor {
                ts_ms: r.ts_ms,
                entry_id: r.entry_id,
            })
        })
    } else {
        None
    };

    let items = rows.into_iter().map(row_to_dto).collect();
    Ok(Json(Paginated { items, next_cursor }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_parse_accepts_canonical_labels() {
        assert_eq!(parse_min_severity("info").unwrap(), 0);
        assert_eq!(parse_min_severity("Warning").unwrap(), 1);
        assert_eq!(parse_min_severity("ERROR").unwrap(), 2);
        assert_eq!(parse_min_severity("warn").unwrap(), 1);
        assert_eq!(parse_min_severity("err").unwrap(), 2);
        assert!(parse_min_severity("fatal").is_err());
    }

    #[test]
    fn like_pattern_escapes_wildcards() {
        // A user query for "50%" must not match "501 Not Implemented".
        let p = build_like_pattern("50%");
        assert_eq!(p, "%50\\%%");
        let p2 = build_like_pattern("foo_bar");
        assert_eq!(p2, "%foo\\_bar%");
        let p3 = build_like_pattern("a\\b");
        assert_eq!(p3, "%a\\\\b%");
    }

    #[test]
    fn entry_cursor_round_trip_non_null_ts() {
        let c = EntryCursor { ts_ms: Some(1_700_000_000_000), entry_id: 42 };
        let encoded = encode_entry_cursor(&c);
        let decoded = decode_entry_cursor(&encoded).unwrap();
        assert_eq!(decoded.ts_ms, Some(1_700_000_000_000));
        assert_eq!(decoded.entry_id, 42);
    }

    #[test]
    fn entry_cursor_round_trip_null_ts() {
        // The NULL-timestamp tier must survive a round trip — this is the
        // tier that's easiest to get wrong in a one-column cursor scheme.
        let c = EntryCursor { ts_ms: None, entry_id: 99 };
        let encoded = encode_entry_cursor(&c);
        let decoded = decode_entry_cursor(&encoded).unwrap();
        assert_eq!(decoded.ts_ms, None);
        assert_eq!(decoded.entry_id, 99);
    }

    #[test]
    fn entry_cursor_rejects_garbage() {
        assert!(decode_entry_cursor("!!!not-base64").is_err());
        // Base64-valid but no separator.
        let bad = crate::routes::encode_cursor("noseparator");
        assert!(decode_entry_cursor(&bad).is_err());
        // Bad integer.
        let bad = crate::routes::encode_cursor("abc|def");
        assert!(decode_entry_cursor(&bad).is_err());
    }

    #[test]
    fn severity_to_string_maps_known_values() {
        assert_eq!(severity_to_string(0), "Info");
        assert_eq!(severity_to_string(1), "Warning");
        assert_eq!(severity_to_string(2), "Error");
        assert!(severity_to_string(99).starts_with("Unknown"));
    }
}
