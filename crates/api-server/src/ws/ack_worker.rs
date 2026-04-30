//! Drains the request_ack mpsc and writes ACK / COMPLETE state into PG.

use crate::storage::MetadataStore;
use common_wire::ws::{AgentFrame, RequestOutcome};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

pub async fn run(
    meta: Arc<dyn MetadataStore>,
    mut rx: mpsc::Receiver<AgentFrame>,
) {
    while let Some(frame) = rx.recv().await {
        match frame {
            AgentFrame::RequestAck { request_id, accepted, .. } => {
                if let Err(e) = meta.record_request_ack(request_id, accepted).await {
                    warn!(?request_id, error = %e, "record_request_ack failed");
                }
            }
            AgentFrame::RequestComplete { request_id, bundle_id, outcome, error, .. } => {
                let outcome_str = match outcome {
                    RequestOutcome::Ok => "ok",
                    RequestOutcome::Error => "error",
                };
                if let Err(e) = meta.record_request_complete(
                    request_id,
                    Some(bundle_id),
                    outcome_str,
                    error.as_deref(),
                )
                .await
                {
                    warn!(?request_id, error = %e, "record_request_complete failed");
                }
            }
            _ => {}
        }
    }
}
