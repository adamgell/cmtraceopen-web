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

use crate::auth::{AuthMode, AuthState, EntraConfig, JwksCache};
use crate::storage::{BlobStore, MetadataStore};

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
    /// Operator-bearer auth configuration + JWKS cache. Consumed by the
    /// `OperatorPrincipal` extractor on query routes.
    pub auth: AuthState,
}

impl AppState {
    /// Build the full shared state with auth enabled. `listen_addr` is the
    /// stringified bind address used purely for display on the status page.
    pub fn new(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        auth: AuthState,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            started_at: Instant::now(),
            request_count: AtomicU64::new(0),
            listen_addr,
            hostname: detect_hostname(),
            auth,
        })
    }

    /// Test-only shortcut: build state with auth disabled and no Entra
    /// config. Integration tests that don't exercise the auth surface
    /// prefer this over hand-rolling a full `AuthState`.
    pub fn new_auth_disabled(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
    ) -> Arc<Self> {
        let auth = AuthState {
            mode: AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        };
        Self::new(meta, blobs, listen_addr, auth)
    }

    /// Test helper: build state with auth enabled, pointed at a caller-
    /// supplied JWKS cache (typically pre-seeded with a hand-minted pubkey).
    pub fn new_with_auth(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        entra: EntraConfig,
        jwks: Arc<JwksCache>,
    ) -> Arc<Self> {
        let auth = AuthState {
            mode: AuthMode::Enabled,
            entra: Some(entra),
            jwks,
        };
        Self::new(meta, blobs, listen_addr, auth)
    }
}

/// Best-effort hostname lookup. Uses `HOSTNAME` / `COMPUTERNAME` env vars to
/// avoid pulling in a platform-specific crate for a debug-only field.
fn detect_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
