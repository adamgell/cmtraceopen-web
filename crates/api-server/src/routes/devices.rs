//! Device registry query routes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use common_wire::{DeviceSummary, Paginated};

use crate::error::AppError;
use crate::routes::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/devices", get(list_devices))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

async fn list_devices(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Paginated<DeviceSummary>>, AppError> {
    let limit = clamp_limit(q.limit, DEFAULT_LIMIT, MAX_LIMIT);
    // Request one extra row to decide whether another page exists. Cursor
    // payload is simply the last device_id on the returned page.
    let after = q.cursor.as_deref().map(decode_cursor).transpose()?;

    let mut rows = state
        .meta
        .list_devices(limit + 1, after.as_deref())
        .await?;

    let next_cursor = if rows.len() as u32 > limit {
        let sentinel = rows.pop().expect("we just checked len > limit");
        // Keep the `limit` rows we'll actually return; encode sentinel's
        // predecessor (last in the truncated set) as the cursor.
        rows.last().map(|r| encode_cursor(&r.device_id)).or_else(|| {
            // defensive: shouldn't happen since limit >= 1
            Some(encode_cursor(&sentinel.device_id))
        })
    } else {
        None
    };

    let items = rows
        .into_iter()
        .map(|r| DeviceSummary {
            device_id: r.device_id,
            first_seen_utc: r.first_seen_utc,
            last_seen_utc: r.last_seen_utc,
            hostname: r.hostname,
            session_count: r.session_count,
        })
        .collect();

    Ok(Json(Paginated { items, next_cursor }))
}
