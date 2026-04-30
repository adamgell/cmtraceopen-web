//! `POST /v1/devices/{device_id}/request-bundle` — operator-driven bundle
//! request. Looks up the device's WS terminator and dispatches the
//! request_bundle frame either locally or via /v1/internal/forward.

use crate::state::AppState;
use crate::storage::{BundleRequestSource, NewBundleRequest};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use common_wire::ws::{BundleReason, ServerFrame};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RequestBody {
    #[serde(default)]
    pub operator_email: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct ResponseBody {
    pub request_id: Uuid,
    pub device: String,
    pub status: &'static str,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Json(body): Json<RequestBody>,
) -> Result<(StatusCode, Json<ResponseBody>), (StatusCode, String)> {
    if !state
        .meta
        .agent_exists(&device_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::NOT_FOUND, "device not registered".into()));
    }

    let request_id = Uuid::new_v4();
    let row = NewBundleRequest {
        request_id,
        device_id: device_id.clone(),
        source: BundleRequestSource::Operator,
        schedule_name: None,
        operator_email: body.operator_email,
        requested_at: Utc::now(),
    };
    state
        .meta
        .insert_bundle_request(row)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let replica = state
        .meta
        .lookup_connection(&device_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(replica_id) = replica else {
        state
            .meta
            .mark_bundle_request_offline(request_id)
            .await
            .ok();
        return Err((StatusCode::CONFLICT, "device offline".into()));
    };

    let frame = ServerFrame::RequestBundle {
        request_id,
        reason: BundleReason::Operator,
        schedule_name: None,
    };

    if replica_id == state.replica_id {
        if state.ws_registry.try_send(&device_id, frame).await.is_err() {
            state
                .meta
                .mark_bundle_request_offline(request_id)
                .await
                .ok();
            return Err((StatusCode::CONFLICT, "device disconnected".into()));
        }
    } else {
        crate::routes::internal::forward_to_replica(
            &state,
            &replica_id,
            &device_id,
            &frame,
            request_id,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(ResponseBody {
            request_id,
            device: device_id,
            status: "dispatched",
        }),
    ))
}
