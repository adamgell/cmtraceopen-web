//! Per-connection task: spawned for each accepted WebSocket. Owns
//! reading from the socket, dispatching incoming AgentFrames, and
//! writing outbound ServerFrames from the registry's mpsc. Also
//! drives the WS-protocol ping/pong timer (axum doesn't auto-send).

use crate::ws::auth::{verify_frame_device_id, WsAuthError};
use crate::ws::ConnectionRegistry;
use axum::extract::ws::{Message, WebSocket};
use common_wire::ws::{AgentFrame, ServerFrame};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);
pub const PING_INTERVAL: Duration = Duration::from_secs(30);
pub const PONG_TIMEOUT: Duration = Duration::from_secs(60);

/// Outbound channel from the connection task to whatever consumes
/// agent-side events (heartbeat persister, request-ack handler).
#[derive(Clone)]
pub struct InboundSink {
    pub heartbeats: mpsc::Sender<common_wire::ws::Heartbeat>,
    pub request_acks: mpsc::Sender<AgentFrame>,
}

pub async fn run_connection(
    device_id: String,
    socket: WebSocket,
    registry: ConnectionRegistry,
    inbound: InboundSink,
) {
    let (mut sink, mut stream) = socket.split();
    let mut outbound_rx = registry.insert(device_id.clone()).await;

    let mut last_heartbeat = Instant::now();
    let mut last_pong = Instant::now();
    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    use futures::{SinkExt, StreamExt};
    loop {
        tokio::select! {
            Some(frame) = outbound_rx.recv() => {
                let json = match serde_json::to_string(&frame) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(%device_id, error = %e, "failed to serialize ServerFrame");
                        continue;
                    }
                };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    info!(%device_id, "outbound write failed; closing");
                    break;
                }
            }

            Some(msg) = stream.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => { info!(%device_id, error = %e, "ws read error"); break; }
                };
                match msg {
                    Message::Text(t) => {
                        let frame: AgentFrame = match serde_json::from_str(t.as_str()) {
                            Ok(f) => f,
                            Err(e) => {
                                warn!(%device_id, error = %e, "bad frame; closing 1003");
                                let _ = sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                                    code: 1003, reason: "unsupported data".into(),
                                }))).await;
                                break;
                            }
                        };
                        if let Err(e) = verify_frame_device_id(&device_id, &frame) {
                            warn!(%device_id, error = %e, "device_id mismatch; closing 1008");
                            let _ = sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                                code: 1008, reason: "policy violation".into(),
                            }))).await;
                            break;
                        }
                        match frame {
                            AgentFrame::Heartbeat(hb) => {
                                last_heartbeat = Instant::now();
                                let ack = ServerFrame::HeartbeatAck { ts: chrono::Utc::now() };
                                if let Ok(json) = serde_json::to_string(&ack) {
                                    let _ = sink.send(Message::Text(json.into())).await;
                                }
                                let _ = inbound.heartbeats.try_send(hb);
                            }
                            f @ (AgentFrame::RequestAck { .. } | AgentFrame::RequestComplete { .. }) => {
                                let _ = inbound.request_acks.try_send(f);
                            }
                        }
                    }
                    Message::Pong(_) => {
                        last_pong = Instant::now();
                    }
                    Message::Close(_) => {
                        debug!(%device_id, "agent closed");
                        break;
                    }
                    _ => {}
                }
            }

            _ = ping_ticker.tick() => {
                if sink.send(Message::Ping(vec![].into())).await.is_err() {
                    info!(%device_id, "ping write failed; closing");
                    break;
                }
                if last_pong.elapsed() > PONG_TIMEOUT {
                    info!(%device_id, "pong timeout; closing");
                    break;
                }
                if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
                    info!(%device_id, "heartbeat timeout; closing");
                    break;
                }
            }
        }
    }

    registry.remove(&device_id).await;
}

#[allow(dead_code)]
fn _suppress_unused(_: WsAuthError) {}
