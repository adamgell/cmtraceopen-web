//! Admin-only routes.
//!
//! Routes here are gated on [`RequireRole<AdminTag>`][crate::auth::RequireRole]
//! — i.e. the caller's JWT must carry the `CmtraceOpen.Admin` app role.
//! Operator-level tokens (delegated `CmtraceOpen.Query` scope or the
//! `CmtraceOpen.Operator` app role) are explicitly NOT accepted here, so
//! that destructive admin actions can never be invoked through the
//! interactive web viewer's user-delegated token.
//!
//! ## Audit logging
//!
//! Every request that reaches a handler in this module is wrapped by
//! [`crate::middleware::audit::audit_middleware`], which appends one row to
//! `audit_log` after the handler returns — regardless of whether the handler
//! succeeded.  `GET /v1/admin/audit` itself is excluded from self-auditing.
//!
//! ## Routes
//!
//!   POST /v1/admin/devices/{device_id}/disable  →  501 Not Implemented (reserved)
//!   GET  /v1/admin/audit                        →  200 + paginated audit rows
//!
//! Adding more admin routes? Wire them through this same router so the
//! `RequireRole<AdminTag>` discipline and audit middleware are uniform.
//!
//! See `docs/provisioning/02-entra-app-registration.md` §6 for how the
//! admin app role is defined in Entra and how operators get assigned to it
//! via the Enterprise Application's Users-and-groups blade.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use chrono::{DateTime, Utc};
use common_wire::Paginated;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{AdminTag, RequireRole};
use crate::error::AppError;
use crate::middleware::audit::audit_middleware;
use crate::routes::{decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::storage::AuditFilters;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/v1/admin/devices/{device_id}/disable",
            post(disable_device),
        )
        .route("/v1/admin/audit", get(list_audit))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            audit_middleware,
        ))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Disable device (placeholder)
// ---------------------------------------------------------------------------

/// Placeholder admin route. Returns 501 Not Implemented. Exists so:
///   1. The URL + verb are reserved (clients can stub against it).
///   2. The `RequireRole<AdminTag>` extractor is exercised in integration
///      tests, locking in the 401/403/501 status matrix.
///   3. The audit middleware is exercised: a 501 response produces an
///      audit row with `result=failure`.
///   4. The OpenAPI surface (added in a later PR) has something concrete to
///      point at when documenting the admin role.
async fn disable_device(
    _principal: RequireRole<AdminTag>,
    Path(device_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let body = serde_json::json!({
        "error": "not_implemented",
        "message": format!(
            "device-disable is not yet implemented; reserved admin route for device '{device_id}'"
        ),
    });
    (StatusCode::NOT_IMPLEMENTED, Json(body))
}

// ---------------------------------------------------------------------------
// Audit log read endpoint
// ---------------------------------------------------------------------------

/// Query parameters accepted by `GET /v1/admin/audit`.
///
/// Pagination uses an opaque `cursor` token returned by the previous
/// page's `next_cursor` field — see [`AuditFilters::cursor_before`] for
/// the keyset semantics. Clients should treat the cursor as opaque and
/// pass it back verbatim.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Opaque cursor from a previous page's `next_cursor`. Omit to start
    /// from the most recent rows.
    pub cursor: Option<String>,
    /// Filter to a specific `principal_id` (JWT `sub`).
    pub principal: Option<String>,
    /// Filter to a specific action string (e.g. `device.disable`).
    pub action: Option<String>,
    /// Maximum number of rows to return. Clamped to 1 000.
    pub limit: Option<u32>,
}

/// Wire-format for a single audit row in the list response.
#[derive(Debug, Serialize)]
pub struct AuditRowDto {
    pub id: String,
    pub ts_utc: DateTime<Utc>,
    pub principal_kind: String,
    pub principal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_display: Option<String>,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_json: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `GET /v1/admin/audit` — keyset-paginated, reverse-chronological audit
/// history.
///
/// Callers must hold the `CmtraceOpen.Admin` role.  Query parameters:
///   - `cursor`    — opaque cursor from a previous page's `next_cursor`
///   - `principal` — filter to a specific principal_id
///   - `action`    — filter to a specific action string
///   - `limit`     — max rows (1–1 000; default 100)
///
/// Returns `{ items: [...], next_cursor: "..." | null }` — clients pass
/// `next_cursor` back as `cursor=` to fetch the next page. Matches the
/// pagination convention used by `routes/sessions.rs`.
async fn list_audit(
    _principal: RequireRole<AdminTag>,
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQuery>,
) -> Result<Json<Paginated<AuditRowDto>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 1000);

    // Decode the opaque cursor into the (ts, id) keyset pair the storage
    // layer expects. Cursor format: base64("<rfc3339>|<uuid>") — same
    // shape as routes/sessions.rs uses.
    let cursor_before = params
        .cursor
        .as_deref()
        .map(|c| {
            let decoded = decode_cursor(c)?;
            let (ts_str, id_str) = decoded
                .split_once('|')
                .ok_or_else(|| AppError::BadRequest("invalid audit cursor payload".into()))?;
            let ts = DateTime::parse_from_rfc3339(ts_str)
                .map_err(|_| AppError::BadRequest("invalid audit cursor timestamp".into()))?
                .with_timezone(&Utc);
            let id = Uuid::parse_str(id_str)
                .map_err(|_| AppError::BadRequest("invalid audit cursor uuid".into()))?;
            Ok::<_, AppError>((ts, id))
        })
        .transpose()?;

    let filters = AuditFilters {
        cursor_before,
        principal: params.principal,
        action: params.action,
    };

    // Over-fetch by 1 so we can detect "more rows available" → emit a
    // next_cursor. Same trick routes/sessions.rs uses.
    let mut rows = state
        .audit
        .list_audit_rows(&filters, limit + 1)
        .await?;

    let next_cursor = if rows.len() as u32 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| {
            encode_cursor(&format!("{}|{}", r.ts_utc.to_rfc3339(), r.id))
        })
    } else {
        None
    };

    let items: Vec<AuditRowDto> = rows
        .into_iter()
        .map(|r| AuditRowDto {
            id: r.id.to_string(),
            ts_utc: r.ts_utc,
            principal_kind: r.principal_kind,
            principal_id: r.principal_id,
            principal_display: r.principal_display,
            action: r.action,
            target_kind: r.target_kind,
            target_id: r.target_id,
            result: r.result,
            details_json: r.details_json.as_deref().and_then(|s| {
                serde_json::from_str(s).ok()
            }),
            request_id: r.request_id.map(|u| u.to_string()),
        })
        .collect();

    Ok(Json(Paginated { items, next_cursor }))
}
