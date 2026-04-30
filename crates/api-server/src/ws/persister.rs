//! Drains heartbeats from a bounded mpsc and writes to PG. Drop-oldest
//! semantics so a slow DB doesn't backpressure WS reads.

use crate::storage::MetadataStore;
use common_wire::ws::Heartbeat;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

pub const PERSISTER_BUFFER: usize = 256;

pub fn channel() -> (mpsc::Sender<Heartbeat>, mpsc::Receiver<Heartbeat>) {
    mpsc::channel(PERSISTER_BUFFER)
}

pub async fn run(meta: Arc<dyn MetadataStore>, mut rx: mpsc::Receiver<Heartbeat>) {
    while let Some(hb) = rx.recv().await {
        if let Err(e) = meta.upsert_agent_and_heartbeat(&hb).await {
            warn!(device_id = %hb.device_id, error = %e, "heartbeat persist failed");
            metrics::counter!("cmtrace_heartbeat_persist_errors_total").increment(1);
        }
    }
}

/// Caller-side helper: tries to send, drops oldest on full. Increments
/// the drops metric so we can detect overload.
pub fn try_enqueue(tx: &mpsc::Sender<Heartbeat>, hb: Heartbeat) {
    use mpsc::error::TrySendError;
    match tx.try_send(hb) {
        Ok(_) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {
            metrics::counter!("cmtrace_heartbeat_drops_total").increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn hb(device_id: &str) -> Heartbeat {
        Heartbeat {
            device_id: device_id.into(),
            device_name: "n".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "win".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 0,
            disk_free_pct: 0,
            uptime_seconds: 0,
        }
    }

    #[tokio::test]
    async fn try_enqueue_drops_when_full() {
        let (tx, _rx) = mpsc::channel::<Heartbeat>(2);
        try_enqueue(&tx, hb("a"));
        try_enqueue(&tx, hb("b"));
        try_enqueue(&tx, hb("c")); // should silently drop, not panic
    }
}
