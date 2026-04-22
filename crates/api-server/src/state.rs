//! Shared application state injected into handlers via `State`.
//!
//! Holds the two storage traits as trait objects so handlers don't care
//! whether the backend is local-fs + SQLite (MVP) or S3 + Postgres (later).
//!
//! Also carries a handful of process-wide fields surfaced on the dev status
//! page (`GET /`): monotonic start time, a per-route request counter bumped
//! by the counter middleware, the listen address, and the host name. These
//! are intentionally parked on the same struct so every handler sees a single
//! unified state type rather than juggling multiple `State<T>` extractors.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

use crate::auth::{AuthMode, AuthState, EntraConfig, JwksCache};
#[cfg(feature = "crl")]
use crate::auth::CrlCache;
use crate::storage::{BlobStore, MetadataStore};

/// Subset of [`crate::config::TlsConfig`] the request-handling layer needs
/// at runtime: which scheme to expect on the SAN URI and whether ingest
/// routes should reject requests that arrive without a verified client
/// cert. The cert/key/CA paths live only in the startup path.
#[derive(Debug, Clone)]
pub struct MtlsRuntimeConfig {
    /// Mirror of [`crate::config::TlsConfig::require_on_ingest`].
    pub require_on_ingest: bool,
    /// Mirror of [`crate::config::TlsConfig::expected_san_uri_scheme`].
    pub expected_san_uri_scheme: String,
}

impl Default for MtlsRuntimeConfig {
    fn default() -> Self {
        Self {
            require_on_ingest: false,
            expected_san_uri_scheme: "device".to_string(),
        }
    }
}

/// Per-route request counter map, keyed by the Axum `MatchedPath` template
/// (e.g. `/v1/sessions/{id}/entries`, not the substituted path). The
/// request-counter middleware bumps the bucket for the matched route on
/// every request; the dev status page sweeps the map to render a top-N
/// table.
///
/// `DashMap` shards internally so two requests on different routes don't
/// contend on a single mutex. Each value is an `AtomicU64` so bumps inside
/// a shard are still lock-free reads on the shard map. Sentinel key
/// `unmatched` collects 404s where no route template was matched.
pub type RouteCounterMap = DashMap<String, AtomicU64>;

/// Sentinel key used by the request-counter middleware when a request didn't
/// match any registered route (i.e. would 404). Surfaced verbatim in the
/// status-page table so operators can spot unexpected probe traffic.
pub const UNMATCHED_ROUTE_KEY: &str = "unmatched";

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
    /// Per-route HTTP request count since process start, keyed by the Axum
    /// `MatchedPath` route template (so `/v1/sessions/{id}/entries` counts
    /// once per route, not per id). Bumped by the request-counter middleware
    /// once per request; swept and sorted by the status page handler.
    pub request_counts: Arc<RouteCounterMap>,
    /// Listen address copied from Config at startup — cheap to stringify.
    pub listen_addr: String,
    /// Hostname reported by the kernel at startup (best-effort; falls back to
    /// `"unknown"` if the OS lookup fails).
    pub hostname: String,
    /// Operator-bearer auth configuration + JWKS cache. Consumed by the
    /// `OperatorPrincipal` extractor on query routes.
    pub auth: AuthState,
    /// CORS settings threaded into the outer layer at router-build time. Kept
    /// on `AppState` so tests and `main.rs` share a single construction path.
    pub cors: CorsConfig,
    /// mTLS runtime knobs consumed by the `DeviceIdentity` extractor on
    /// ingest routes. Defaults to "no enforcement", which preserves the
    /// legacy `X-Device-Id` header-only behavior.
    pub mtls: MtlsRuntimeConfig,
    /// Client-cert revocation cache. Populated + refreshed by the
    /// background task spawned in `main.rs`. Threaded into the
    /// `DeviceIdentity` extractor (PR #41 follow-up) so revoked certs
    /// reject with 401. `None` means CRL polling was either compiled out
    /// (`--no-default-features`) or not configured (no
    /// `CMTRACE_CRL_URLS`); the extractor falls through to allow-all in
    /// that case, matching the pre-CRL posture.
    #[cfg(feature = "crl")]
    pub crl_cache: Option<Arc<CrlCache>>,
}

impl AppState {
    /// Build the full shared state with auth enabled. `listen_addr` is the
    /// stringified bind address used purely for display on the status page.
    ///
    /// Defaults to an empty CORS allowed-origins list (fail-closed). Call
    /// sites that need CORS (the real `main.rs`, the CORS integration tests)
    /// use [`AppState::with_cors`] instead.
    pub fn new(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        auth: AuthState,
    ) -> Arc<Self> {
        Self::with_cors(meta, blobs, listen_addr, auth, CorsConfig::default())
    }

    /// Same as [`AppState::new`] but with an explicit CORS config.
    pub fn with_cors(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
    ) -> Arc<Self> {
        Self::full(meta, blobs, listen_addr, auth, cors, MtlsRuntimeConfig::default())
    }

    /// Build the shared state with explicit CORS + mTLS knobs. The full
    /// constructor used by `main.rs`; tests usually go through `new` /
    /// `with_cors` and pick up the default (mTLS off) variant.
    pub fn full(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            started_at: Instant::now(),
            request_counts: Arc::new(DashMap::new()),
            listen_addr,
            hostname: detect_hostname(),
            auth,
            cors,
            mtls,
            #[cfg(feature = "crl")]
            crl_cache: None,
        })
    }

    /// Same as [`AppState::full`] but lets `main.rs` install a pre-built
    /// CRL cache. Kept as a separate constructor to avoid disturbing
    /// existing test call sites that already pass through
    /// [`AppState::with_cors`] / [`AppState::new`].
    #[cfg(feature = "crl")]
    pub fn with_cors_and_crl(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
        crl_cache: Option<Arc<CrlCache>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            started_at: Instant::now(),
            request_count: AtomicU64::new(0),
            listen_addr,
            hostname: detect_hostname(),
            auth,
            cors,
            mtls,
            crl_cache,
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
