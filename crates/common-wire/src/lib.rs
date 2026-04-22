// common-wire
//
// Shared protocol types (DTOs) used by the api-server, the future Windows
// agent, and eventually the web viewer. Platform-agnostic, wasm-safe, no
// Tauri or native dependencies.
//
// All DTOs serialize with camelCase field names so they're idiomatic on the
// wire for TypeScript clients and still ergonomic in Rust.

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod ingest {
    //! Bundle-ingest wire protocol (Phase 3 M1).
    //!
    //! Flow:
    //!   1. Agent POSTs `BundleInitRequest` → server returns `BundleInitResponse`
    //!      with an upload_id, server-chosen chunk size, and any resume offset.
    //!   2. Agent PUTs chunks to `/v1/ingest/bundles/{upload_id}/chunks?offset=N`.
    //!   3. Agent POSTs `BundleFinalizeRequest` with the final sha256; server
    //!      verifies and returns `BundleFinalizeResponse` with the new session_id.
    use super::*;

    /// Content that a bundle upload carries. Drives server-side parsing later.
    pub mod content_kind {
        /// A full evidence zip collected by the on-device agent.
        pub const EVIDENCE_ZIP: &str = "evidence-zip";
        /// Pre-parsed NDJSON entries (one LogEntry per line).
        pub const NDJSON_ENTRIES: &str = "ndjson-entries";
        /// A single raw file (e.g. a single CMTrace .log).
        pub const RAW_FILE: &str = "raw-file";
    }

    /// Agent → server: start a new bundle upload.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BundleInitRequest {
        /// Stable per collection run. Lets the agent retry init without
        /// creating duplicate sessions: a (device_id, bundle_id) already
        /// present short-circuits to its existing session on finalize.
        pub bundle_id: Uuid,
        /// Device hint for pre-registration. Ignored once mTLS lands in M2;
        /// until then the authoritative device identity comes from the
        /// `X-Device-Id` header.
        pub device_hint: Option<String>,
        /// Hex-encoded sha256 of the full bundle.
        pub sha256: String,
        /// Total bundle size in bytes.
        pub size_bytes: u64,
        /// One of the constants in [`content_kind`].
        pub content_kind: String,
    }

    /// Server → agent: accept an upload; tell the agent how to chunk it.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BundleInitResponse {
        pub upload_id: Uuid,
        /// Server-chosen chunk size (bytes). Agent SHOULD use this size.
        /// MVP default: 8 MiB.
        pub chunk_size: u64,
        /// 0 on a fresh upload; non-zero if a previous upload for the same
        /// (device_id, bundle_id) was interrupted and we can resume.
        pub resume_offset: u64,
    }

    /// Server → agent: chunk accepted; here's the byte offset to send next.
    /// Returned from `PUT /v1/ingest/bundles/{upload_id}/chunks?offset=N`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ChunkUploadResponse {
        /// Byte offset immediately after the bytes the server just committed.
        /// Clients should send the next chunk at this offset.
        pub next_offset: u64,
    }

    /// Agent → server: finalize an upload.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BundleFinalizeRequest {
        /// Hex-encoded sha256 the agent computed over the full bundle.
        /// Server recomputes over the staged file and rejects on mismatch.
        pub final_sha256: String,
    }

    /// Server → agent: upload committed; here's the session_id for queries.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BundleFinalizeResponse {
        pub session_id: Uuid,
        /// Parse state for the bundle's contents. MVP always returns
        /// `"pending"` — a background parser lands in M2.
        pub parse_state: String,
    }
}

pub mod registry {
    //! Device + session query DTOs surfaced to operators / the viewer.
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DeviceSummary {
        pub device_id: String,
        pub first_seen_utc: DateTime<Utc>,
        pub last_seen_utc: DateTime<Utc>,
        pub hostname: Option<String>,
        pub session_count: i64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SessionSummary {
        pub session_id: Uuid,
        pub device_id: String,
        pub bundle_id: Uuid,
        pub collected_utc: Option<DateTime<Utc>>,
        pub ingested_utc: DateTime<Utc>,
        pub size_bytes: u64,
        pub parse_state: String,
    }
}

pub mod query {
    //! Per-session entry + file query DTOs.
    //!
    //! These intentionally mirror only the fields surfaced on the wire, not
    //! `cmtraceopen-parser::LogEntry` verbatim. Keeping the wire DTO flat +
    //! self-contained lets the web/api side evolve independently of the
    //! desktop parser crate (which carries many format-specific fields and
    //! would otherwise bloat every response payload).
    use super::*;

    /// One row from the `files` table: a single raw log file that was
    /// extracted out of a bundle and parsed.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FileSummary {
        pub file_id: String,
        pub session_id: String,
        pub relative_path: String,
        pub size_bytes: u64,
        pub format_detected: Option<String>,
        pub parser_kind: Option<String>,
        pub entry_count: u64,
        pub parse_error_count: u64,
    }

    /// One parsed log entry, flattened for the viewer API.
    ///
    /// `extras` is an opaque JSON object surfacing format-specific fields
    /// (`http_method`, `result_code`, IIS verb, etc.) without committing the
    /// wire to the desktop parser's rich `LogEntry` enum. Clients that care
    /// about a specific field can look it up by name.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct LogEntryDto {
        pub entry_id: i64,
        pub file_id: String,
        pub line_number: u32,
        pub ts_ms: Option<i64>,
        /// Enum-like string: `"Info"` | `"Warning"` | `"Error"`.
        pub severity: String,
        pub component: Option<String>,
        pub thread: Option<String>,
        pub message: String,
        pub extras: Option<serde_json::Value>,
    }
}

pub mod config {
    //! Server-side config push (Wave 4).
    //!
    //! [`AgentConfigOverride`] is the subset of agent config fields that are
    //! safe to override from the server — fields that could brick the agent on
    //! mis-config (`api_endpoint`, TLS cert paths) are deliberately excluded.
    //!
    //! Flow:
    //!   1. Operator issues `PUT /v1/admin/devices/{device_id}/config` or
    //!      `PUT /v1/admin/config/default` with a JSON body of this shape.
    //!   2. Agent fetches `GET /v1/config/{device_id}` at startup, every 6 h,
    //!      and after each successful upload.
    //!   3. Agent merges the returned overrides on top of its local config.
    //!   4. If the agent fails to connect for 24 h after applying an override,
    //!      it reverts to the last-known-good local config.
    use serde::{Deserialize, Serialize};

    /// Operator-managed config overrides pushed to a device (or all devices).
    ///
    /// All fields are `Option<_>` so a partial override payload only touches
    /// the keys that are present; missing keys leave the local-config value
    /// intact.
    ///
    /// **Not overridable (safety boundary):** `api_endpoint`, TLS cert/key/CA
    /// paths — changing those remotely could permanently disconnect the agent.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AgentConfigOverride {
        /// `tracing` filter directive, e.g. `"debug"` or `"info"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub log_level: Option<String>,

        /// HTTP request timeout in seconds (must be ≥ 1).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub request_timeout_secs: Option<u64>,

        /// Cron expression for the evidence collector, e.g. `"0 3 * * *"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub evidence_schedule: Option<String>,

        /// Maximum bundles the on-disk upload queue will hold (must be ≥ 1).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub queue_max_bundles: Option<usize>,

        /// Log paths the `logs` collector will walk. Replaces the full list
        /// when set.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub log_paths: Option<Vec<String>>,
    }

    impl AgentConfigOverride {
        /// Returns `true` if every field is `None` (i.e. a no-op override).
        pub fn is_empty(&self) -> bool {
            self.log_level.is_none()
                && self.request_timeout_secs.is_none()
                && self.evidence_schedule.is_none()
                && self.queue_max_bundles.is_none()
                && self.log_paths.is_none()
        }

        /// Validate that overridable numeric fields are within acceptable
        /// bounds.  Returns `Err` with a human-readable message for the
        /// first failing field, `Ok(())` if everything looks sane.
        pub fn validate(&self) -> Result<(), String> {
            if let Some(t) = self.request_timeout_secs {
                if t == 0 {
                    return Err(
                        "request_timeout_secs must be ≥ 1; zero would immediately time out every request".into(),
                    );
                }
                if t > 3600 {
                    return Err(format!(
                        "request_timeout_secs {t} exceeds the safety cap of 3600 s"
                    ));
                }
            }
            if let Some(q) = self.queue_max_bundles {
                if q == 0 {
                    return Err(
                        "queue_max_bundles must be ≥ 1; zero would discard every bundle immediately".into(),
                    );
                }
                if q > 10_000 {
                    return Err(format!(
                        "queue_max_bundles {q} exceeds the safety cap of 10 000"
                    ));
                }
            }
            Ok(())
        }
    }
}

pub use config::AgentConfigOverride;

/// Generic keyset-paginated envelope. `next_cursor` is an opaque, base64 token
/// that clients pass back verbatim to fetch the next page. `None` means no
/// more results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Response body for `GET /healthz` and `GET /readyz`.
///
/// Shallow liveness payload. `service` + `version` come from the server's
/// `CARGO_PKG_NAME` / `CARGO_PKG_VERSION` at compile time; `status` is
/// `"ok"` for a healthy process. Typed so downstream clients (the agent's
/// health probe, the web viewer, ops tooling) can deserialize without
/// dropping to `serde_json::Value`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
}

/// JSON body emitted by the api-server's `AppError::into_response`.
///
/// `error` is a stable, snake_case code (`bad_request`, `not_found`,
/// `offset_mismatch`, ...) — clients branch on this. `message` is a
/// human-readable explanation built from the error's `Display` impl and is
/// intentionally not part of any machine contract (it may change wording
/// between versions).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

// Re-exports so downstream crates can use the flat path if they prefer.
pub use ingest::{
    BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest, BundleInitResponse,
    ChunkUploadResponse,
};
pub use query::{FileSummary, LogEntryDto};
pub use registry::{DeviceSummary, SessionSummary};
// `HealthResponse` + `ErrorBody` live at the crate root; no alias needed.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_init_request_uses_camel_case() {
        let req = BundleInitRequest {
            bundle_id: Uuid::nil(),
            device_hint: Some("WIN-ABC".into()),
            sha256: "deadbeef".into(),
            size_bytes: 1024,
            content_kind: ingest::content_kind::EVIDENCE_ZIP.into(),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("bundleId").is_some());
        assert!(v.get("deviceHint").is_some());
        assert!(v.get("sizeBytes").is_some());
        assert!(v.get("contentKind").is_some());
        assert!(v.get("size_bytes").is_none(), "snake_case should not appear");
    }

    #[test]
    fn file_summary_uses_camel_case() {
        let fs = query::FileSummary {
            file_id: "f1".into(),
            session_id: "s1".into(),
            relative_path: "a.log".into(),
            size_bytes: 10,
            format_detected: Some("cmtrace".into()),
            parser_kind: Some("cmtrace".into()),
            entry_count: 3,
            parse_error_count: 0,
        };
        let v = serde_json::to_value(&fs).unwrap();
        assert!(v.get("fileId").is_some());
        assert!(v.get("sessionId").is_some());
        assert!(v.get("relativePath").is_some());
        assert!(v.get("sizeBytes").is_some());
        assert!(v.get("formatDetected").is_some());
        assert!(v.get("parserKind").is_some());
        assert!(v.get("entryCount").is_some());
        assert!(v.get("parseErrorCount").is_some());
    }

    #[test]
    fn log_entry_dto_uses_camel_case_and_supports_extras() {
        let e = query::LogEntryDto {
            entry_id: 7,
            file_id: "f1".into(),
            line_number: 42,
            ts_ms: Some(1_700_000_000_000),
            severity: "Info".into(),
            component: Some("ccmexec".into()),
            thread: None,
            message: "hello".into(),
            extras: Some(serde_json::json!({ "httpMethod": "GET" })),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert!(v.get("entryId").is_some());
        assert!(v.get("fileId").is_some());
        assert!(v.get("lineNumber").is_some());
        assert!(v.get("tsMs").is_some());
        assert_eq!(v.get("extras").unwrap().get("httpMethod").unwrap(), "GET");
    }

    #[test]
    fn health_response_round_trips() {
        let h = HealthResponse {
            status: "ok".into(),
            service: "cmtraceopen-api".into(),
            version: "0.1.0".into(),
        };
        let s = serde_json::to_string(&h).unwrap();
        // All three fields happen to be single-word, so camelCase == snake_case
        // here; the test exists to lock the shape against accidental rename.
        assert!(s.contains("\"status\":\"ok\""));
        assert!(s.contains("\"service\":\"cmtraceopen-api\""));
        assert!(s.contains("\"version\":\"0.1.0\""));
        let back: HealthResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.status, "ok");
        assert_eq!(back.service, "cmtraceopen-api");
        assert_eq!(back.version, "0.1.0");
    }

    #[test]
    fn error_body_round_trips() {
        let e = ErrorBody {
            error: "bad_request".into(),
            message: "missing X-Device-Id header".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"error\":\"bad_request\""));
        assert!(s.contains("\"message\":\"missing X-Device-Id header\""));
        let back: ErrorBody = serde_json::from_str(&s).unwrap();
        assert_eq!(back.error, "bad_request");
        assert_eq!(back.message, "missing X-Device-Id header");
    }

    #[test]
    fn agent_config_override_camel_case_and_validate() {
        let over = AgentConfigOverride {
            log_level: Some("debug".into()),
            request_timeout_secs: Some(30),
            evidence_schedule: Some("0 6 * * *".into()),
            queue_max_bundles: Some(20),
            log_paths: Some(vec!["C:\\Logs\\**\\*.log".into()]),
        };
        let v = serde_json::to_value(&over).unwrap();
        assert!(v.get("logLevel").is_some(), "camelCase logLevel missing");
        assert!(v.get("requestTimeoutSecs").is_some());
        assert!(v.get("evidenceSchedule").is_some());
        assert!(v.get("queueMaxBundles").is_some());
        assert!(v.get("logPaths").is_some());
        // snake_case must not appear
        assert!(v.get("log_level").is_none());

        let back: AgentConfigOverride = serde_json::from_value(v).unwrap();
        assert_eq!(back, over);
        assert!(over.validate().is_ok());
    }

    #[test]
    fn agent_config_override_is_empty() {
        assert!(AgentConfigOverride::default().is_empty());
        let non_empty = AgentConfigOverride {
            log_level: Some("info".into()),
            ..AgentConfigOverride::default()
        };
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn agent_config_override_validate_rejects_bad_values() {
        let bad_timeout = AgentConfigOverride {
            request_timeout_secs: Some(0),
            ..AgentConfigOverride::default()
        };
        assert!(bad_timeout.validate().is_err());

        let bad_queue = AgentConfigOverride {
            queue_max_bundles: Some(0),
            ..AgentConfigOverride::default()
        };
        assert!(bad_queue.validate().is_err());
    }

    #[test]
    fn agent_config_override_skip_none_fields() {
        // Only set one field; the JSON should only contain that field.
        let over = AgentConfigOverride {
            log_level: Some("warn".into()),
            ..AgentConfigOverride::default()
        };
        let v = serde_json::to_value(&over).unwrap();
        assert!(v.get("logLevel").is_some());
        // Fields that are None should be omitted from JSON (skip_serializing_if)
        assert!(v.get("requestTimeoutSecs").is_none());
        assert!(v.get("queueMaxBundles").is_none());
    }

    #[test]
    fn paginated_round_trips() {
        let p = Paginated::<String> {
            items: vec!["a".into(), "b".into()],
            next_cursor: Some("Y3Vyc29y".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("nextCursor"));
        let back: Paginated<String> = serde_json::from_str(&s).unwrap();
        assert_eq!(back.items, vec!["a", "b"]);
        assert_eq!(back.next_cursor.as_deref(), Some("Y3Vyc29y"));
    }
}
