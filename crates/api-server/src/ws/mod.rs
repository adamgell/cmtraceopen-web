//! Per-replica WebSocket subsystem. Holds the in-memory map from
//! `device_id` to outbound mpsc; spawns the per-connection handler tasks.

pub mod ack_worker;
pub mod auth;
pub mod handler;
pub mod persister;
#[cfg(feature = "postgres")]
pub mod sweeper;

use common_wire::ws::ServerFrame;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Bounded buffer per connection's outbound mpsc. A slow agent applies
/// backpressure to dispatchers; a stalled write closes the connection.
pub const OUTBOUND_BUFFER: usize = 16;

/// Shared per-replica registry. Cloned via the surrounding `Arc`.
#[derive(Clone, Default)]
pub struct ConnectionRegistry {
    inner: Arc<RwLock<HashMap<String, mpsc::Sender<ServerFrame>>>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly-connected device. Returns the receiver for the
    /// per-connection write task. If a stale registration exists, evict
    /// it (drops the old sender → old write task observes channel close
    /// → old conn shuts down).
    pub async fn insert(&self, device_id: String) -> mpsc::Receiver<ServerFrame> {
        let (tx, rx) = mpsc::channel(OUTBOUND_BUFFER);
        self.inner.write().await.insert(device_id, tx);
        rx
    }

    /// Remove a device when its connection closes.
    pub async fn remove(&self, device_id: &str) {
        self.inner.write().await.remove(device_id);
    }

    /// Try to dispatch a frame to the device. Returns Ok if the frame was
    /// queued, Err if the device is not connected to *this* replica or the
    /// outbound buffer is full (slow agent).
    pub async fn try_send(
        &self,
        device_id: &str,
        frame: ServerFrame,
    ) -> Result<(), DispatchError> {
        let map = self.inner.read().await;
        let tx = map.get(device_id).ok_or(DispatchError::NotLocal)?;
        tx.try_send(frame).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => DispatchError::Backpressure,
            mpsc::error::TrySendError::Closed(_) => DispatchError::NotLocal,
        })
    }

    /// Snapshot of currently-registered device_ids on this replica. Used
    /// only for tests / metrics.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn device_ids(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("device is not connected to this replica")]
    NotLocal,
    #[error("outbound buffer full (agent is slow)")]
    Backpressure,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn req() -> ServerFrame {
        ServerFrame::HeartbeatAck { ts: Utc::now() }
    }

    #[tokio::test]
    async fn insert_remove_round_trip() {
        let r = ConnectionRegistry::new();
        let _rx = r.insert("dev-1".into()).await;
        assert_eq!(r.device_ids().await, vec!["dev-1".to_string()]);
        r.remove("dev-1").await;
        assert!(r.device_ids().await.is_empty());
    }

    #[tokio::test]
    async fn try_send_to_unknown_returns_not_local() {
        let r = ConnectionRegistry::new();
        let result = r.try_send("ghost", req()).await;
        assert!(matches!(result, Err(DispatchError::NotLocal)));
    }

    #[tokio::test]
    async fn try_send_full_buffer_returns_backpressure() {
        let r = ConnectionRegistry::new();
        let mut _rx = r.insert("dev-1".into()).await;
        for _ in 0..OUTBOUND_BUFFER {
            r.try_send("dev-1", req()).await.unwrap();
        }
        let result = r.try_send("dev-1", req()).await;
        assert!(matches!(result, Err(DispatchError::Backpressure)));
    }

    #[tokio::test]
    async fn closed_receiver_reads_as_not_local() {
        let r = ConnectionRegistry::new();
        let rx = r.insert("dev-1".into()).await;
        drop(rx);
        let result = r.try_send("dev-1", req()).await;
        assert!(matches!(result, Err(DispatchError::NotLocal)));
    }

    #[tokio::test]
    async fn re_insert_evicts_old_sender() {
        let r = ConnectionRegistry::new();
        let mut rx1 = r.insert("dev-1".into()).await;
        let _rx2 = r.insert("dev-1".into()).await;
        assert!(rx1.recv().await.is_none(), "old receiver should observe close");
    }

    fn _allow_unused() {
        let _ = Uuid::nil();
    }
}
