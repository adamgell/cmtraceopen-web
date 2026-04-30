//! Bundle = one unit of work the harness sends to a target. Plain data so
//! it crosses async boundaries cheaply.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One bundle ready to be shipped to a Target. The harness produces these
/// (synthetically or replayed); the target consumes them.
#[derive(Clone, Debug)]
pub struct Bundle {
    /// Sequence number within the run; uniquely identifies the bundle in
    /// `bundles.jsonl`.
    pub seq: u64,
    /// Logical device this bundle is "from" (`LOAD-TEST-<run>-<seq%n>`).
    pub device_id: String,
    /// Wire-protocol bundle id (uuid v7) — used by the agent's finalize
    /// payload.
    pub bundle_id: uuid::Uuid,
    /// Number of log files inside the zip. Recorded for analysis.
    pub n_files: u32,
    /// Raw zip bytes. Always ≤ MAX_EVIDENCE_ZIP_BYTES (50 MiB).
    pub zip_bytes: Vec<u8>,
}

/// Per-bundle outcome. Written to `bundles.jsonl` by the reporter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleResult {
    pub seq: u64,
    pub device: String,
    pub n_files: u32,
    pub bundle_bytes: u64,
    /// Time from "start sending" to "finalize response received".
    /// `None` for in-process target (no HTTP layer).
    pub finalize_ms: Option<u64>,
    /// Time from finalize to `parse_state != "pending"`. Always populated
    /// (the harness polls until terminal).
    pub parse_ms: u64,
    pub parse_state: String,
    pub files_with_fallbacks: Option<u32>,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

impl BundleResult {
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.finalize_ms.unwrap_or(0) + self.parse_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_result_serializes_with_expected_keys() {
        let r = BundleResult {
            seq: 42,
            device: "LOAD-TEST-A-0001".into(),
            n_files: 24,
            bundle_bytes: 4_812_000,
            finalize_ms: Some(120),
            parse_ms: 3450,
            parse_state: "ok-with-fallbacks".into(),
            files_with_fallbacks: Some(3),
            http_status: Some(201),
            error: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["seq"], 42);
        assert_eq!(v["device"], "LOAD-TEST-A-0001");
        assert_eq!(v["parse_state"], "ok-with-fallbacks");
        assert_eq!(v["finalize_ms"], 120);
        assert_eq!(v["error"], serde_json::Value::Null);
    }

    #[test]
    fn elapsed_sums_finalize_and_parse() {
        let r = BundleResult {
            seq: 0,
            device: "d".into(),
            n_files: 0,
            bundle_bytes: 0,
            finalize_ms: Some(100),
            parse_ms: 200,
            parse_state: "ok".into(),
            files_with_fallbacks: None,
            http_status: None,
            error: None,
        };
        assert_eq!(r.elapsed(), Duration::from_millis(300));
    }
}
