//! `/v1/schedules` — operator-managed cron schedules.

use crate::schedule::selector;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub cron: String,
    pub selector: Value,
    pub rate_pct: i32,
    #[serde(default)]
    pub jitter_seconds: i32,
    #[serde(default)]
    pub cooldown_seconds: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}
fn default_enabled() -> bool { true }

#[derive(Serialize, sqlx::FromRow)]
pub struct ScheduleView {
    pub name: String,
    pub cron: String,
    pub selector_json: Value,
    pub rate_pct: i32,
    pub jitter_seconds: i32,
    pub cooldown_seconds: i32,
    pub enabled: bool,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub next_fire_at: DateTime<Utc>,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<ScheduleView>), (StatusCode, String)> {
    let cron = CronSchedule::from_str(&body.cron)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid cron: {e}")))?;
    let next_fire = cron.upcoming(Utc).next()
        .ok_or((StatusCode::BAD_REQUEST, "cron yields no next time".into()))?;
    selector::render(&body.selector)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid selector: {e}")))?;
    if !(1..=100).contains(&body.rate_pct) {
        return Err((StatusCode::BAD_REQUEST, "rate_pct must be 1..100".into()));
    }
    let pool = pool_of(&state)?;
    sqlx::query(
        "INSERT INTO schedules (name, cron, selector_json, rate_pct, jitter_seconds,
                                cooldown_seconds, enabled, next_fire_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"
    )
    .bind(&body.name).bind(&body.cron).bind(&body.selector)
    .bind(body.rate_pct).bind(body.jitter_seconds).bind(body.cooldown_seconds)
    .bind(body.enabled).bind(next_fire)
    .execute(pool).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let view = sqlx::query_as::<_, ScheduleView>(
        "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                cooldown_seconds, enabled, last_fired_at, next_fire_at
         FROM schedules WHERE name = $1"
    ).bind(&body.name).fetch_one(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((StatusCode::CREATED, Json(view)))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ScheduleView>>, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, ScheduleView>(
        "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                cooldown_seconds, enabled, last_fired_at, next_fire_at
         FROM schedules ORDER BY name"
    ).fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let res = sqlx::query("DELETE FROM schedules WHERE name = $1")
        .bind(&name).execute(pool).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "schedule not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn pool_of(state: &Arc<AppState>) -> Result<&sqlx::PgPool, (StatusCode, String)> {
    state.meta.pg_pool()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "schedules require PG store".into()))
}
