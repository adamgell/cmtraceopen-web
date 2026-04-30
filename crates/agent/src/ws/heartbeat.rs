//! Sends a heartbeat AgentFrame every 45 seconds. Snapshot of agent state
//! is collected via the `Snapshot` trait; the binary supplies a
//! `RuntimeSnapshot` that reads real values, tests supply mocks.

use chrono::{DateTime, Utc};
use common_wire::ws::{AgentFrame, Heartbeat};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(45);

#[async_trait::async_trait]
pub trait Snapshot: Send + Sync + 'static {
    async fn snapshot(&self) -> SnapshotData;
}

#[derive(Debug, Clone)]
pub struct SnapshotData {
    pub device_id: String,
    pub device_name: String,
    pub intune_device_id: Option<String>,
    pub ninjaone_device_id: Option<String>,
    pub asset_tag: Option<String>,
    pub agent_version: String,
    pub os_version: String,
    pub last_collect_at: Option<DateTime<Utc>>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

pub async fn run(snap: impl Snapshot, tx: mpsc::Sender<AgentFrame>) {
    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.tick().await; // immediate first tick
    loop {
        ticker.tick().await;
        let s = snap.snapshot().await;
        let frame = AgentFrame::Heartbeat(Heartbeat {
            device_id: s.device_id,
            device_name: s.device_name,
            intune_device_id: s.intune_device_id,
            ninjaone_device_id: s.ninjaone_device_id,
            asset_tag: s.asset_tag,
            ts: Utc::now(),
            agent_version: s.agent_version,
            os_version: s.os_version,
            last_collect_at: s.last_collect_at,
            queue_depth: s.queue_depth,
            errors_24h: s.errors_24h,
            disk_free_pct: s.disk_free_pct,
            uptime_seconds: s.uptime_seconds,
        });
        if let Err(e) = tx.send(frame).await {
            warn!(error = %e, "heartbeat send failed; ws probably restarting");
        }
    }
}
