//! Session query routes.
//!
//!   GET /v1/devices/{device_id}/sessions?limit=&cursor=
//!   GET /v1/sessions/{session_id}
//!
//! Cursor for the list endpoint is base64("<rfc3339>|<uuid>"). Clients treat
//! it as opaque.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::DateTime;
use serde::Deserialize;
use uuid::Uuid;

use common_wire::{Paginated, SessionSummary};

use crate::error::AppError;
use crate::routes::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::storage::SessionRow;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/devices/{device_id}/sessions", get(list_for_device))
        .route("/v1/sessions/{session_id}", get(get_session))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

fn row_to_summary(r: SessionRow) -> SessionSummary {
    SessionSummary {
        session_id: r.session_id,
        device_id: r.device_id,
        bundle_id: r.bundle_id,
        collected_utc: r.collected_utc,
        ingested_utc: r.ingested_utc,
        size_bytes: r.size_bytes,
        parse_state: r.parse_state,
    }
}

async fn list_for_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Paginated<SessionSummary>>, AppError> {
    let limit = clamp_limit(q.limit, DEFAULT_LIMIT, MAX_LIMIT);

    let before = q
        .cursor
        .as_deref()
        .map(|c| {
            let decoded = decode_cursor(c)?;
            let (ts_str, uuid_str) = decoded.split_once('|').ok_or_else(|| {
                AppError::BadRequest("invalid cursor payload".into())
            })?;
            let ts = DateTime::parse_from_rfc3339(ts_str)
                .map_err(|_| AppError::BadRequest("invalid cursor timestamp".into()))?
                .with_timezone(&chrono::Utc);
            let uid = Uuid::parse_str(uuid_str)
                .map_err(|_| AppError::BadRequest("invalid cursor uuid".into()))?;
            Ok::<_, AppError>((ts, uid))
        })
        .transpose()?;

    let mut rows = state
        .meta
        .list_sessions_for_device(&device_id, limit + 1, before)
        .await?;

    let next_cursor = if rows.len() as u32 > limit {
        rows.truncate(limit as usize);
        rows.last()
            .map(|r| encode_cursor(&format!("{}|{}", r.ingested_utc.to_rfc3339(), r.session_id)))
    } else {
        None
    };

    let items = rows.into_iter().map(row_to_summary).collect();
    Ok(Json(Paginated { items, next_cursor }))
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionSummary>, AppError> {
    let row = state
        .meta
        .get_session(session_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("session {session_id} not found")))?;
    Ok(Json(row_to_summary(row)))
}
