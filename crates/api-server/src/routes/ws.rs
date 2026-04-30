//! `/v1/agent/ws` — WebSocket upgrade endpoint. Authenticates via
//! `X-Device-Id` header, then hands off to the per-connection handler.

use crate::state::AppState;
use crate::ws::auth::extract_device_id;
use crate::ws::handler::{run_connection, InboundSink};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{routing::get, Router};
use chrono::Utc;
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/agent/ws", get(upgrade))
        .with_state(state)
}

async fn upgrade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let device_id = extract_device_id(&headers)
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    let Some(pool) = state.meta.pg_pool() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "WS endpoint requires PG metadata store".into(),
        ));
    };

    let now = Utc::now();
    if let Err(e) = sqlx::query(
        "INSERT INTO connections (device_id, replica_id, connected_at, last_heartbeat_at, disconnected_at)
         VALUES ($1, $2, $3, $3, NULL)
         ON CONFLICT (device_id) DO UPDATE SET
           replica_id = EXCLUDED.replica_id,
           connected_at = EXCLUDED.connected_at,
           last_heartbeat_at = EXCLUDED.last_heartbeat_at,
           disconnected_at = NULL",
    )
    .bind(&device_id)
    .bind(&state.replica_id)
    .bind(now)
    .execute(pool)
    .await
    {
        tracing::warn!(error = %e, "failed to record connection row; rejecting upgrade");
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("connection record failed: {e}"),
        ));
    }

    let registry = state.ws_registry.clone();
    let inbound = InboundSink {
        heartbeats: state.heartbeat_tx.clone(),
        request_acks: state.request_ack_tx.clone(),
    };
    Ok(ws
        .on_upgrade(move |socket| async move {
            run_connection(device_id, socket, registry, inbound).await;
        })
        .into_response())
}
