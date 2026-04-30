//! Target = the system under test. Three implementations:
//! in-process (direct `parse_session` call), local-http (managed compose
//! stack), remote-http (external URL).

pub mod in_process;
pub mod local_http;
pub mod remote_http;

use crate::bundle::{Bundle, BundleResult};

#[async_trait::async_trait]
pub trait Target: Send + Sync {
    /// Send one bundle through the target's full path. Returns a result
    /// row ready to be JSON-serialized into bundles.jsonl.
    async fn send(&self, bundle: Bundle) -> BundleResult;

    /// Sample target system metrics (in-flight count, RSS, DB pool stats).
    /// Returns a JSON object that the system sampler appends as one row.
    async fn sample_system(&self) -> serde_json::Value;

    /// Tear down anything the target brought up (compose, scratch DB, etc.).
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()>;
}
