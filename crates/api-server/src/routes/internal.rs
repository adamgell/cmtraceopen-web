//! Internal cross-replica forward endpoint. Real impl in Task 11.

use crate::state::AppState;
use common_wire::ws::ServerFrame;
use std::sync::Arc;
use uuid::Uuid;

pub async fn forward_to_replica(
    _state: &Arc<AppState>,
    _replica_id: &str,
    _device_id: &str,
    _frame: &ServerFrame,
    _request_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // TODO(Task 11): real HTTP forward with single retry. Stub returns Ok
    // for now so the operator endpoint compiles before cross-replica
    // routing lands.
    Ok(())
}
