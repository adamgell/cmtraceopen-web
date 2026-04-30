//! WS connection lifecycle with exponential reconnect.

use common_wire::ws::{AgentFrame, ServerFrame};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use url::Url;

const RECONNECT_BACKOFFS: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];

pub struct WsClientHandles {
    /// Channel for AgentFrames the agent wants to send (heartbeats, ACKs).
    pub outbound_tx: mpsc::Sender<AgentFrame>,
    /// Channel of ServerFrames received from the server.
    pub inbound_rx: mpsc::Receiver<ServerFrame>,
}

/// Run the connect/reconnect loop forever. Returns a pair of channels for
/// the rest of the agent to use; the channels survive reconnects.
pub fn spawn(
    base_url: String,
    device_id: String,
) -> WsClientHandles {
    let (out_tx, mut out_rx) = mpsc::channel::<AgentFrame>(32);
    let (in_tx, in_rx) = mpsc::channel::<ServerFrame>(32);
    let handles = WsClientHandles { outbound_tx: out_tx.clone(), inbound_rx: in_rx };

    tokio::spawn(async move {
        let mut backoff_idx = 0;
        loop {
            match connect_and_run(&base_url, &device_id, &mut out_rx, &in_tx).await {
                Ok(()) => {
                    info!("ws closed cleanly; reconnecting");
                    backoff_idx = 0;
                }
                Err(e) => {
                    warn!(error = %e, "ws connection error");
                    let dur = RECONNECT_BACKOFFS[backoff_idx];
                    tokio::time::sleep(dur).await;
                    if backoff_idx + 1 < RECONNECT_BACKOFFS.len() {
                        backoff_idx += 1;
                    }
                }
            }
        }
    });
    handles
}

async fn connect_and_run(
    base_url: &str,
    device_id: &str,
    out_rx: &mut mpsc::Receiver<AgentFrame>,
    in_tx: &mpsc::Sender<ServerFrame>,
) -> anyhow::Result<()> {
    use futures::{SinkExt, StreamExt};
    let mut url = Url::parse(base_url)?.join("/v1/agent/ws")?;
    // tokio-tungstenite requires the WebSocket URL scheme. Same endpoint
    // exposed under ws/wss; we just rewrite the scheme that came from the
    // agent's `api_endpoint` (which uses http/https for the ingest path).
    let _ = url.set_scheme(match url.scheme() {
        "https" => "wss",
        _ => "ws",
    });
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut().insert(
        "x-device-id",
        HeaderValue::from_str(device_id)?,
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(req).await?;
    info!("ws connected");
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            Some(frame) = out_rx.recv() => {
                let json = serde_json::to_string(&frame)?;
                sink.send(Message::Text(json)).await?;
            }
            msg = stream.next() => {
                let msg = msg.ok_or_else(|| anyhow::anyhow!("ws closed"))??;
                match msg {
                    Message::Text(t) => {
                        let f: ServerFrame = serde_json::from_str(t.as_str())?;
                        let _ = in_tx.send(f).await;
                    }
                    Message::Ping(p) => {
                        sink.send(Message::Pong(p)).await?;
                    }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}
