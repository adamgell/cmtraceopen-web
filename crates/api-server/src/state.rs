//! Shared application state injected into handlers via `State`.
//!
//! Holds the two storage traits as trait objects so handlers don't care
//! whether the backend is local-fs + SQLite (MVP) or S3 + Postgres (later).
//!
//! Also carries a handful of process-wide fields surfaced on the dev status
//! page (`GET /`): monotonic start time, a request counter bumped by the
//! counter middleware, the listen address, and the host name. These are
//! intentionally parked on the same struct so every handler sees a single
//! unified state type rather than juggling multiple `State<T>` extractors.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use crate::storage::{BlobStore, MetadataStore};

/// Subset of [`crate::config::Config`] that the router threads into layers.
///
/// Kept as a small `Clone` struct (rather than an `Arc<Config>`) so tests can
/// build one in-line without going through `Config::from_env`. Populated from
/// the full `Config` in `main.rs` and defaulted (empty list, credentials
/// disabled) in integration tests that don't care about CORS.
#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    /// Exact origins to allow. Empty = fail closed (no cross-origin traffic).
    pub allowed_origins: Vec<String>,
    /// Mirror of [`crate::config::Config::allow_credentials`].
    pub allow_credentials: bool,
}

/// Default chunk size we advertise to agents: 8 MiB. Picked to balance
/// round-trip overhead against memory-per-request. Agents MAY send smaller
/// chunks; the server enforces a hard cap (32 MiB) per chunk regardless.
pub const DEFAULT_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Hard per-chunk cap. Protects the server from OOM if a misbehaving agent
/// sends a multi-GB PUT body.
///
/// This value is the single source of truth for the ingest-route body limit:
/// the Axum router layers [`axum::extract::DefaultBodyLimit::max`] at
/// exactly this size on the ingest sub-router so it can never drift from the
/// runtime check in the chunk handler.
pub const MAX_CHUNK_SIZE: u64 = 32 * 1024 * 1024;

pub struct AppState {
    pub meta: Arc<dyn MetadataStore>,
    pub blobs: Arc<dyn BlobStore>,
    /// Monotonic start time; used by the status page for uptime math.
    pub started_at: Instant,
    /// Total HTTP requests served since process start, all routes + methods.
    /// Incremented once per request by the request-counter middleware.
    pub request_count: AtomicU64,
    /// Listen address copied from Config at startup — cheap to stringify.
    pub listen_addr: String,
    /// Hostname reported by the kernel at startup (best-effort; falls back to
    /// `"unknown"` if the OS lookup fails).
    pub hostname: String,
    /// CORS settings threaded into the outer layer at router-build time. Kept
    /// on `AppState` so tests and `main.rs` share a single construction path.
    pub cors: CorsConfig,
}

impl AppState {
    /// Build the full shared state. `listen_addr` is the stringified bind
    /// address used purely for display on the status page.
    ///
    /// Integration tests that don't exercise CORS can call this helper; it
    /// defaults to an empty allowed-origins list (fail-closed). Call sites
    /// that need CORS (the real `main.rs`, the CORS integration tests) use
    /// [`AppState::with_cors`] instead.
    pub fn new(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
    ) -> Arc<Self> {
        Self::with_cors(meta, blobs, listen_addr, CorsConfig::default())
    }

    /// Same as [`AppState::new`] but with an explicit CORS config.
    pub fn with_cors(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        cors: CorsConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            started_at: Instant::now(),
            request_count: AtomicU64::new(0),
            listen_addr,
            hostname: detect_hostname(),
            cors,
        })
    }
}

/// Best-effort hostname lookup. Uses `HOSTNAME` / `COMPUTERNAME` env vars to
/// avoid pulling in a platform-specific crate for a debug-only field.
fn detect_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
