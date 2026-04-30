//! Cross-replica forwarding. POST /v1/internal/forward accepts a frame +
//! device_id from a peer replica and drops it onto the local registry.
//! Sender side (`forward_to_replica`) does single retry + 500ms backoff.

use crate::state::AppState;
use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use common_wire::ws::ServerFrame;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MAX_FORWARD_ATTEMPTS: i32 = 2;
const FORWARD_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Deserialize, Serialize)]
pub struct ForwardBody {
    pub device_id: String,
    pub frame: ServerFrame,
}

/// Receiving side: drop the frame onto our local registry.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForwardBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .ws_registry
        .try_send(&body.device_id, body.frame)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Sender side: HTTP POST to the peer replica with single retry.
pub async fn forward_to_replica(
    state: &Arc<AppState>,
    replica_id: &str,
    device_id: &str,
    frame: &ServerFrame,
    request_id: Uuid,
) -> Result<()> {
    let url = replica_url(replica_id);
    let body = ForwardBody {
        device_id: device_id.to_string(),
        frame: frame.clone(),
    };

    for attempt in 0..MAX_FORWARD_ATTEMPTS {
        let _ = state.meta.bump_forward_attempts(request_id).await;
        match state.http_client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                tracing::warn!(
                    replica_id,
                    %url,
                    status = ?resp.status(),
                    "forward attempt {} failed",
                    attempt + 1
                );
            }
            Err(e) => {
                tracing::warn!(
                    replica_id,
                    %url,
                    error = %e,
                    "forward attempt {} errored",
                    attempt + 1
                );
            }
        }
        if attempt + 1 < MAX_FORWARD_ATTEMPTS {
            tokio::time::sleep(FORWARD_BACKOFF).await;
        }
    }
    state
        .meta
        .mark_bundle_request_offline(request_id)
        .await
        .ok();
    anyhow::bail!(
        "all {MAX_FORWARD_ATTEMPTS} forward attempts failed for {replica_id}"
    )
}

fn replica_url(replica_id: &str) -> String {
    let host = std::env::var("CMTRACE_REPLICA_HOST_TEMPLATE")
        .unwrap_or_else(|_| "{replica}.cmtrace-internal:8080".into());
    let host = host.replace("{replica}", replica_id);
    format!("http://{host}/v1/internal/forward")
}
