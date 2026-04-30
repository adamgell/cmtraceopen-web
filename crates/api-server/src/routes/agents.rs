//! `/v1/agents` and `/v1/agents/{id}/forget`.

use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize, sqlx::FromRow)]
pub struct AgentView {
    pub device_id: String,
    pub device_name: String,
    pub intune_device_id: Option<String>,
    pub ninjaone_device_id: Option<String>,
    pub asset_tag: Option<String>,
    pub agent_version: String,
    pub os_version: String,
    pub last_seen_at: DateTime<Utc>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AgentView>>, (StatusCode, String)> {
    let pool = state.meta.pg_pool()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "agents endpoint requires PG store".into()))?;
    let rows = sqlx::query_as::<_, AgentView>(
        "SELECT device_id, device_name, intune_device_id, ninjaone_device_id,
                asset_tag, agent_version, os_version, last_seen_at,
                queue_depth, errors_24h, disk_free_pct, uptime_seconds
         FROM agents ORDER BY last_seen_at DESC LIMIT 1000"
    ).fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

pub async fn forget(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = state.meta.pg_pool()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "forget endpoint requires PG store".into()))?;
    let res = sqlx::query("DELETE FROM agents WHERE device_id = $1")
        .bind(&device_id).execute(pool).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "agent not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}
