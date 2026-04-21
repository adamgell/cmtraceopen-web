//! Per-session file-listing route.
//!
//!   GET /v1/sessions/{session_id}/files?limit=&cursor=
//!
//! Returns a keyset-paginated list of [`FileSummary`] rows — one per file
//! the parser emitted into the `files` table for the given session. The
//! parse-on-ingest sister PR populates that table during bundle ingest.
//!
//! Cursor: opaque base64 of the last returned `file_id`. UUIDv7 file_ids
//! sort in insertion time order, so ascending keyset on that column is a
//! clean forward walk.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use common_wire::{FileSummary, Paginated};

use crate::error::AppError;
use crate::routes::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::storage::FileRow;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/sessions/{session_id}/files", get(list_files))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 500;

fn row_to_summary(r: FileRow) -> FileSummary {
    FileSummary {
        file_id: r.file_id,
        session_id: r.session_id,
        relative_path: r.relative_path,
        size_bytes: r.size_bytes,
        format_detected: r.format_detected,
        parser_kind: r.parser_kind,
        entry_count: r.entry_count,
        parse_error_count: r.parse_error_count,
    }
}

#[tracing::instrument(skip_all, fields(%session_id, limit = ?q.limit, has_cursor = q.cursor.is_some()))]
async fn list_files(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Paginated<FileSummary>>, AppError> {
    // Validate session exists so clients get a clean 404 instead of an empty
    // page they might mistake for "parser still running".
    if state.meta.get_session(session_id).await?.is_none() {
        return Err(AppError::NotFound(format!(
            "session {session_id} not found"
        )));
    }

    let limit = clamp_limit(q.limit, DEFAULT_LIMIT, MAX_LIMIT);
    let after = q.cursor.as_deref().map(decode_cursor).transpose()?;

    // Fetch limit+1 to determine if another page exists without a count query.
    let mut rows = state
        .meta
        .list_files_for_session(session_id, limit + 1, after.as_deref())
        .await?;

    let next_cursor = if rows.len() as u32 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| encode_cursor(&r.file_id))
    } else {
        None
    };

    let items = rows.into_iter().map(row_to_summary).collect();
    Ok(Json(Paginated { items, next_cursor }))
}
