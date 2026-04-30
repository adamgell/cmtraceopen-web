//! Receives ServerFrame::RequestBundle, runs the existing collection +
//! upload path, sends RequestAck immediately and RequestComplete when done.

use common_wire::ws::{AgentFrame, RequestOutcome, ServerFrame};
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait BundleRunner: Send + Sync + 'static {
    /// Runs the existing collection + upload pipeline. Returns the
    /// bundle_id committed at finalize.
    async fn run(&self, request_id: Uuid) -> anyhow::Result<Uuid>;
    fn device_id(&self) -> String;
    fn device_name(&self) -> String;
}

pub async fn run<R: BundleRunner>(
    runner: R,
    mut inbound: mpsc::Receiver<ServerFrame>,
    outbound: mpsc::Sender<AgentFrame>,
) {
    while let Some(frame) = inbound.recv().await {
        match frame {
            ServerFrame::RequestBundle { request_id, .. } => {
                info!(?request_id, "received request_bundle");
                let _ = outbound.send(AgentFrame::RequestAck {
                    device_id: runner.device_id(),
                    device_name: runner.device_name(),
                    request_id,
                    accepted: true,
                }).await;
                let result = runner.run(request_id).await;
                let frame = match result {
                    Ok(bundle_id) => AgentFrame::RequestComplete {
                        device_id: runner.device_id(),
                        device_name: runner.device_name(),
                        request_id,
                        bundle_id,
                        outcome: RequestOutcome::Ok,
                        error: None,
                    },
                    Err(e) => AgentFrame::RequestComplete {
                        device_id: runner.device_id(),
                        device_name: runner.device_name(),
                        request_id,
                        bundle_id: Uuid::nil(),
                        outcome: RequestOutcome::Error,
                        error: Some(format!("{e:#}")),
                    },
                };
                if let Err(e) = outbound.send(frame).await {
                    warn!(error = %e, "request_complete send failed");
                }
            }
            ServerFrame::HeartbeatAck { ts } => {
                tracing::debug!(?ts, "heartbeat acked");
            }
        }
    }
}
