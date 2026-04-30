//! `/v1/devices/{id}/bundle-requests` — history of operator + scheduled
//! requests for a single device.

use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}
fn default_limit() -> i64 { 50 }

#[derive(Serialize, sqlx::FromRow)]
pub struct RequestView {
    pub request_id: Uuid,
    pub source: String,
    pub schedule_name: Option<String>,
    pub operator_email: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub acked_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub bundle_id: Option<Uuid>,
    pub outcome: Option<String>,
    pub error: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RequestView>>, (StatusCode, String)> {
    let pool = state.meta.pg_pool()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "PG store required".into()))?;
    let rows = sqlx::query_as::<_, RequestView>(
        "SELECT request_id, source, schedule_name, operator_email,
                requested_at, acked_at, completed_at, bundle_id, outcome, error
         FROM bundle_requests
         WHERE device_id = $1
         ORDER BY requested_at DESC
         LIMIT $2"
    ).bind(&device_id).bind(q.limit.clamp(1, 1000))
     .fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}
