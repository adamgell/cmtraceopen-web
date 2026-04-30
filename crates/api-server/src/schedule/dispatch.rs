//! Dispatch a request_bundle frame to the device, locally or via forward.

use crate::routes::internal;
use crate::state::AppState;
use common_wire::ws::{BundleReason, ServerFrame};
use std::sync::Arc;
use uuid::Uuid;

pub async fn dispatch_scheduled_request(
    state: &Arc<AppState>,
    device_id: &str,
    request_id: Uuid,
    schedule_name: &str,
) {
    let frame = ServerFrame::RequestBundle {
        request_id,
        reason: BundleReason::Scheduled,
        schedule_name: Some(schedule_name.to_string()),
    };
    let replica = match state.meta.lookup_connection(device_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(device_id, error = %e, "lookup_connection failed");
            let _ = state.meta.mark_bundle_request_offline(request_id).await;
            return;
        }
    };
    let Some(replica_id) = replica else {
        let _ = state.meta.mark_bundle_request_offline(request_id).await;
        return;
    };
    if replica_id == state.replica_id {
        if state.ws_registry.try_send(device_id, frame).await.is_err() {
            let _ = state.meta.mark_bundle_request_offline(request_id).await;
        }
    } else if let Err(e) = internal::forward_to_replica(
        state,
        &replica_id,
        device_id,
        &frame,
        request_id,
    )
    .await
    {
        tracing::warn!(device_id, error = %e, "scheduled dispatch failed");
    }
}
