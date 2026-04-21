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

/// Generic keyset-paginated envelope. `next_cursor` is an opaque, base64 token
/// that clients pass back verbatim to fetch the next page. `None` means no
/// more results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

// Re-exports so downstream crates can use the flat path if they prefer.
pub use ingest::{
    BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest, BundleInitResponse,
    ChunkUploadResponse,
};
pub use registry::{DeviceSummary, SessionSummary};

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
