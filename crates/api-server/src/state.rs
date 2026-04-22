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
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use ipnet::IpNet;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::auth::{AuthMode, AuthState, EntraConfig, JwksCache};
#[cfg(feature = "crl")]
use crate::auth::CrlCache;
use crate::config::RateLimitConfig;
use crate::storage::{AuditStore, BlobStore, ConfigStore, MetadataStore, NoopAuditStore};

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

/// A single fixed-window bucket for one rate-limit key.
///
/// Reset when `now - window_start >= window`; counter incremented otherwise.
struct WindowEntry {
    window_start: Instant,
    count: u64,
}

/// Hard cap on the number of distinct keys a single [`RateLimiter`] will
/// hold in memory.
///
/// The background GC task in `main.rs` calls [`RateLimiter::purge_expired`]
/// once per minute, but between two GC ticks an attacker can churn keys
/// arbitrarily fast — and the per-IP limiter is exposed to IPv6 source-
/// address rotation where a single attacker can synthesise millions of
/// unique addresses per second. Without a hard cap the limiter itself
/// becomes the DoS vector. 50_000 covers every device fleet + per-AppGW
/// client population we expect, with bounded memory at the worst-case
/// per-entry size.
///
/// When the map crosses this threshold, [`RateLimiter::check`] runs an
/// in-line opportunistic sweep of expired entries. If the sweep doesn't
/// reclaim room, *new* keys are admitted without being inserted (the
/// limiter fails open on cap exhaustion — the alternative would be to
/// fail closed, but a cap-exhaustion DoS that locks out legitimate
/// callers is itself a denial of service). Existing keys remain enforced.
pub const RATE_LIMIT_MAX_KEYS: usize = 50_000;

/// Simple fixed-window rate limiter keyed by an arbitrary string.
///
/// Backed by a [`DashMap`] so concurrent requests on different keys don't
/// contend. Each per-key entry holds the window start time and the request
/// count. When the window expires, the entry is reset and the new window
/// begins.
///
/// The implementation uses a write lock (via `DashMap::entry`) per check so
/// the increment and the threshold comparison are atomic with respect to other
/// concurrent callers for the same key.
///
/// ## Memory management
///
/// Two lines of defence:
///
/// 1. **Background GC** — `main.rs` spawns a Tokio task that calls
///    [`RateLimiter::purge_expired`] once per window duration so entries
///    whose window has elapsed don't hang around forever. This handles
///    the steady-state case (organic growth as device IDs / IPs come and
///    go).
/// 2. **In-line cap** — [`RATE_LIMIT_MAX_KEYS`] bounds the map at any
///    point in time. The hot path checks the cap before inserting a new
///    key; if the cap is reached it runs a synchronous sweep to try to
///    reclaim space, and falls back to "allow without insert" only when
///    that sweep can't free a slot. This handles the burst-attack case
///    (millions of fresh IPv6 addresses inside a single minute).
pub struct RateLimiter {
    windows: DashMap<String, WindowEntry>,
    /// Maximum requests in a single window.
    pub limit: u64,
    /// Duration of one window.
    pub window: Duration,
}

impl RateLimiter {
    /// Create a new limiter with the given window `limit` and `window` size.
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            windows: DashMap::new(),
            limit,
            window,
        }
    }

    /// Check whether `key` is within the rate limit and increment the counter.
    ///
    /// Returns `Ok(())` when the request is allowed.
    /// Returns `Err(retry_after)` when the limit is exceeded; `retry_after`
    /// is the duration until the current window expires.
    ///
    /// Enforces the [`RATE_LIMIT_MAX_KEYS`] cap on first-time insertions:
    /// if the map is at capacity, an in-line sweep runs first; if no slot
    /// is freed, the new key is admitted without being inserted (fail-open
    /// on cap exhaustion). Existing keys remain enforced regardless of
    /// total map size.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        let now = Instant::now();

        // Cap guard: if this is a brand-new key and the map is at capacity,
        // try to reclaim room with an opportunistic sweep before allowing
        // the insert. If the sweep doesn't free a slot, allow the request
        // through without recording it — see RATE_LIMIT_MAX_KEYS doc-comment
        // for the fail-open rationale. The `contains_key` + `entry` pair is
        // not strictly atomic but the cap is a soft hint anyway: small
        // overshoots from concurrent inserts are acceptable.
        if !self.windows.contains_key(key) && self.windows.len() >= RATE_LIMIT_MAX_KEYS {
            self.purge_expired_inline(now);
            if self.windows.len() >= RATE_LIMIT_MAX_KEYS {
                return Ok(());
            }
        }

        let mut entry = self
            .windows
            .entry(key.to_string())
            .or_insert_with(|| WindowEntry { window_start: now, count: 0 });

        if now.duration_since(entry.window_start) >= self.window {
            // Window expired — start a fresh one.
            entry.window_start = now;
            entry.count = 1;
            Ok(())
        } else if entry.count < self.limit {
            entry.count += 1;
            Ok(())
        } else {
            // Compute how long until this window ends.
            let window_end = entry.window_start + self.window;
            let remaining = window_end.saturating_duration_since(now);
            Err(remaining)
        }
    }

    /// In-line variant of [`Self::purge_expired`] that takes the `now`
    /// reading already captured by the caller. Avoids a second
    /// `Instant::now()` syscall on the cap-exhaustion hot path.
    fn purge_expired_inline(&self, now: Instant) {
        self.windows
            .retain(|_, entry| now.duration_since(entry.window_start) < self.window);
    }

    /// Remove all entries whose window has fully elapsed.
    ///
    /// Safe to call concurrently with [`check`] — `DashMap::retain` takes a
    /// shard lock per shard (not the whole map), so live traffic on other
    /// shards is unaffected. Entries that arrive during a `purge_expired` call
    /// may or may not be swept; both outcomes are correct (a fresh entry would
    /// pass `check` on the next call anyway).
    ///
    /// Intended to be called by a background task once per window duration so
    /// the map footprint is bounded by the number of distinct keys seen in any
    /// single window rather than growing unboundedly over the lifetime of the
    /// process.
    pub fn purge_expired(&self) {
        let now = Instant::now();
        self.windows
            .retain(|_, entry| now.duration_since(entry.window_start) < self.window);
    }

    /// Number of live keys in the map. Useful for logging + metrics.
    ///
    /// Clippy's `len-without-is-empty` lint asks for an `is_empty`
    /// companion because a public `len` traditionally implies a
    /// collection-like API. `RateLimiter` is intentionally not a
    /// collection in that sense (the key set is an internal
    /// implementation detail that callers shouldn't iterate); silencing
    /// the lint is the right call rather than exposing emptiness as
    /// part of the API surface.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.windows.len()
    }
}

#[cfg(test)]
mod limiter_cap_tests {
    use super::*;

    #[test]
    fn map_size_is_hard_capped_under_key_churn() {
        // 1µs window so every entry is expired by the time the next check
        // runs — exercises the in-line sweep path that should keep the map
        // size below the cap even under fast key churn.
        let limiter = RateLimiter::new(10, Duration::from_micros(1));

        for i in 0..(RATE_LIMIT_MAX_KEYS + 5_000) {
            let _ = limiter.check(&format!("k-{i}"));
        }

        assert!(
            limiter.len() <= RATE_LIMIT_MAX_KEYS,
            "limiter.len()={} must not exceed RATE_LIMIT_MAX_KEYS={}",
            limiter.len(),
            RATE_LIMIT_MAX_KEYS,
        );
    }

    #[test]
    fn existing_key_at_cap_is_still_enforced() {
        // Long window so entries don't auto-expire during the test.
        let limiter = RateLimiter::new(2, Duration::from_secs(60));

        // Pre-existing key: first hit allowed, count = 1.
        assert!(limiter.check("alice").is_ok());

        // Saturate the map past the cap with junk keys. Sweep can't reclaim
        // (long window), so the cap-exhaustion fail-open path runs.
        for i in 0..(RATE_LIMIT_MAX_KEYS + 200) {
            let _ = limiter.check(&format!("junk-{i}"));
        }

        // Even at cap saturation the pre-existing key is still counted.
        assert!(limiter.check("alice").is_ok()); // 2nd hit, limit=2
        assert!(limiter.check("alice").is_err()); // 3rd hit → 429
    }
}

/// Collection of rate limiters for the three protected scopes.
///
/// Each limiter is optional — `None` means that scope is disabled (either
/// the limit was configured to `0` or this is a test build that doesn't
/// need rate limiting).
pub struct RateLimitState {
    /// Per-device-ID limiter on bundle-ingest routes. Window: 1 hour.
    pub device_ingest: Option<RateLimiter>,
    /// Per-source-IP limiter on `/v1/ingest/*` routes. Window: 1 minute.
    pub ip_ingest: Option<RateLimiter>,
    /// Per-source-IP limiter on query routes. Window: 1 minute.
    pub ip_query: Option<RateLimiter>,
    /// CIDRs trusted to forward the real client IP in `X-Forwarded-For` /
    /// `X-Real-Ip`. Empty means headers are untrusted and the TCP peer
    /// address is used directly for IP-based rate limiting.
    pub trusted_proxy_cidrs: Vec<IpNet>,
}

impl RateLimitState {
    /// Build from the runtime config. Scopes whose limit is `0` become
    /// `None` (disabled).
    pub fn from_config(cfg: &RateLimitConfig) -> Self {
        let make = |limit: u64, window: Duration| {
            (limit > 0).then(|| RateLimiter::new(limit, window))
        };
        Self {
            device_ingest: make(cfg.ingest_per_device_hour, Duration::from_secs(3600)),
            ip_ingest: make(cfg.ingest_per_ip_minute, Duration::from_secs(60)),
            ip_query: make(cfg.query_per_ip_minute, Duration::from_secs(60)),
            trusted_proxy_cidrs: cfg.trusted_proxy_cidrs.clone(),
        }
    }

    /// All scopes disabled — the default for test constructors that don't
    /// exercise rate-limiting behaviour.
    pub fn disabled() -> Self {
        Self {
            device_ingest: None,
            ip_ingest: None,
            ip_query: None,
            trusted_proxy_cidrs: vec![],
        }
    }
}


/// at runtime: which scheme to expect on the SAN URI and whether ingest
/// routes should reject requests that arrive without a verified client
/// cert. The cert/key/CA paths live only in the startup path.
#[derive(Debug, Clone)]
pub struct MtlsRuntimeConfig {
    /// Mirror of [`crate::config::TlsConfig::require_on_ingest`].
    pub require_on_ingest: bool,
    /// Mirror of [`crate::config::TlsConfig::expected_san_uri_scheme`].
    pub expected_san_uri_scheme: String,
    /// Mirror of [`crate::config::TlsConfig::peer_cert_header`].
    /// When set, the `DeviceIdentity` extractor reads the peer cert PEM
    /// from this header name rather than from the in-process TLS layer.
    pub peer_cert_header: Option<String>,
    /// Mirror of [`crate::config::TlsConfig::trusted_proxy_cidr`].
    /// The cert header is only honoured when the request's TCP peer address
    /// falls within this CIDR.
    pub trusted_proxy_cidr: Option<ipnet::IpNet>,
    /// DER-encoded bytes of the trusted CA certs loaded from
    /// `CMTRACE_CLIENT_CA_BUNDLE`. Populated at startup when
    /// `peer_cert_header` is set. The `DeviceIdentity` extractor uses this
    /// to re-validate the cert presented in the header against the same
    /// trust anchors that the in-process TLS path uses, guarding against
    /// misconfigured proxies that forward unverified certs.
    ///
    /// Empty when `peer_cert_header` is `None` (CA validation is then
    /// handled by the rustls TLS layer for the in-process path).
    pub trusted_ca_ders: Vec<Vec<u8>>,
}

impl Default for MtlsRuntimeConfig {
    fn default() -> Self {
        Self {
            require_on_ingest: false,
            expected_san_uri_scheme: "device".to_string(),
            peer_cert_header: None,
            trusted_proxy_cidr: None,
            trusted_ca_ders: vec![],
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
    /// Append-only audit log for admin/operator actions. The production
    /// binary wires in an [`AuditSqliteStore`][crate::storage::AuditSqliteStore];
    /// tests that don't exercise audit use a [`NoopAuditStore`].
    pub audit: Arc<dyn AuditStore>,
    /// Config-override store for the server-side config push feature (Wave 4).
    pub configs: Arc<dyn ConfigStore>,
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
    /// Handle to the process-wide Prometheus recorder, used by the
    /// `/metrics` route to render the text-exposition snapshot. Cloned from
    /// the global [`metrics_handle()`] so every `AppState` (real + test)
    /// renders against the same underlying registry — counters incremented
    /// during a test show up in that test's `/metrics` response.
    pub metrics: PrometheusHandle,
    /// Per-device and per-IP rate-limit state. Shared across the middleware
    /// functions; individual scopes are `None` when that limit is disabled.
    pub rate_limit: Arc<RateLimitState>,
}

/// Process-wide Prometheus recorder + handle.
///
/// `install_recorder()` registers a global recorder that the
/// `metrics::counter!()` / `metrics::histogram!()` / `metrics::gauge!()`
/// macros emit into. It can only be called **once** per process — calling it
/// twice (e.g. across multiple integration tests in the same `cargo test`
/// binary) returns an error. We wrap the install in a [`OnceLock`] so:
///
///   * `main.rs` initializes it explicitly at startup (and gets to log a
///     warning if it ever fails);
///   * Tests calling [`AppState::new`] / [`AppState::new_auth_disabled`] get
///     a handle for free without having to plumb the install themselves.
///
/// The returned handle is `Clone` — cloning is cheap (it bumps an Arc).
fn metrics_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            // Default histogram buckets cover sub-millisecond → multi-second
            // request paths, which fits both the cheap query routes and the
            // chunky ingest finalize path. Tune via separate calls if a
            // specific metric needs different bounds.
            PrometheusBuilder::new()
                .install_recorder()
                .expect("install_recorder failed: a Prometheus recorder was already registered")
        })
        .clone()
}

/// Public accessor for the global Prometheus handle. Exposed so `main.rs`
/// can warm the recorder + log at startup; callers that just need the
/// handle for an `AppState` field should use [`AppState::new`] which calls
/// this internally.
pub fn install_metrics_recorder() -> PrometheusHandle {
    metrics_handle()
}

impl AppState {
    /// Build the full shared state with auth enabled. `listen_addr` is the
    /// stringified bind address used purely for display on the status page.
    ///
    /// Defaults to an empty CORS allowed-origins list (fail-closed) and rate
    /// limiting disabled. Call sites that need CORS use [`AppState::with_cors`];
    /// sites that need rate limiting use [`AppState::full`] directly.
    pub fn new(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
    ) -> Arc<Self> {
        Self::with_cors(meta, blobs, configs, listen_addr, auth, CorsConfig::default())
    }

    /// Same as [`AppState::new`] but with an explicit CORS config.
    pub fn with_cors(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
    ) -> Arc<Self> {
        Self::full(
            meta,
            blobs,
            configs,
            listen_addr,
            auth,
            cors,
            MtlsRuntimeConfig::default(),
            Arc::new(RateLimitState::disabled()),
        )
    }

    /// Build the shared state with explicit CORS, mTLS, and rate-limit knobs.
    ///
    /// This is the canonical production constructor called from `main.rs`.
    /// Tests that don't need rate limiting go through [`AppState::new`] /
    /// [`AppState::with_cors`] / [`AppState::new_auth_disabled`], which all
    /// default to `RateLimitState::disabled()`.
    ///
    /// Defaults to [`NoopAuditStore`] — production callers that need a real
    /// audit log should pass an [`AuditSqliteStore`][crate::storage::AuditSqliteStore]
    /// via the `audit` parameter (see [`AppState::full_with_audit`]).
    #[allow(clippy::too_many_arguments)]
    pub fn full(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
        rate_limit: Arc<RateLimitState>,
    ) -> Arc<Self> {
        Self::full_with_audit(
            meta,
            blobs,
            configs,
            listen_addr,
            auth,
            cors,
            mtls,
            rate_limit,
            Arc::new(NoopAuditStore),
        )
    }

    /// Like [`AppState::full`] but with an explicit [`AuditStore`] backend.
    /// Used by `main.rs` and audit integration tests to wire in a real store.
    #[allow(clippy::too_many_arguments)]
    pub fn full_with_audit(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
        rate_limit: Arc<RateLimitState>,
        audit: Arc<dyn AuditStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            audit,
            configs,
            started_at: Instant::now(),
            request_counts: Arc::new(DashMap::new()),
            listen_addr,
            hostname: detect_hostname(),
            auth,
            cors,
            mtls,
            #[cfg(feature = "crl")]
            crl_cache: None,
            metrics: metrics_handle(),
            rate_limit,
        })
    }

    /// Same as [`AppState::full`] but lets `main.rs` install a pre-built
    /// CRL cache. Kept as a separate constructor to avoid disturbing
    /// existing test call sites that already pass through
    /// [`AppState::with_cors`] / [`AppState::new`].
    ///
    /// `#[allow(clippy::too_many_arguments)]`: this is a wide constructor
    /// that mirrors the `AppState` shape (one arg per pluggable
    /// dependency). Bundling these into a builder would add ~30 lines of
    /// glue for one caller (`main.rs`); the long signature is the simpler
    /// trade-off until/unless a third caller appears.
    #[cfg(feature = "crl")]
    #[allow(clippy::too_many_arguments)]
    pub fn with_cors_and_crl(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
        crl_cache: Option<Arc<CrlCache>>,
        rate_limit: Arc<RateLimitState>,
    ) -> Arc<Self> {
        Self::with_cors_crl_and_audit(
            meta,
            blobs,
            configs,
            listen_addr,
            auth,
            cors,
            mtls,
            crl_cache,
            rate_limit,
            Arc::new(NoopAuditStore),
        )
    }

    /// Full constructor including CRL cache and an explicit [`AuditStore`].
    /// Used by `main.rs` in production.
    ///
    /// Nine positional args is over clippy's `too-many-arguments`
    /// threshold. Refactoring to a builder would touch every call site
    /// for a constructor only `main.rs` actually uses; silencing the
    /// lint locally is the lower-cost choice.
    #[cfg(feature = "crl")]
    #[allow(clippy::too_many_arguments)]
    pub fn with_cors_crl_and_audit(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        auth: AuthState,
        cors: CorsConfig,
        mtls: MtlsRuntimeConfig,
        crl_cache: Option<Arc<CrlCache>>,
        rate_limit: Arc<RateLimitState>,
        audit: Arc<dyn AuditStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            meta,
            blobs,
            audit,
            configs,
            started_at: Instant::now(),
            request_counts: Arc::new(DashMap::new()),
            listen_addr,
            hostname: detect_hostname(),
            auth,
            cors,
            mtls,
            crl_cache,
            metrics: metrics_handle(),
            rate_limit,
        })
    }

    /// Test-only shortcut: build state with auth disabled and no Entra
    /// config. Integration tests that don't exercise the auth surface
    /// prefer this over hand-rolling a full `AuthState`.
    pub fn new_auth_disabled(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
    ) -> Arc<Self> {
        let auth = AuthState {
            mode: AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        };
        Self::new(meta, blobs, configs, listen_addr, auth)
    }

    /// Test helper: build state with auth disabled and explicit rate-limit
    /// config. Used by the rate-limit integration tests to exercise limiting
    /// without standing up an Entra tenant.
    pub fn new_auth_disabled_with_rate_limit(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        listen_addr: String,
        rate_limit: Arc<RateLimitState>,
    ) -> Arc<Self> {
        let auth = AuthState {
            mode: AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        };
        Self::full(
            meta,
            blobs,
            listen_addr,
            auth,
            CorsConfig::default(),
            MtlsRuntimeConfig::default(),
            rate_limit,
        )
    }

    /// Test helper: build state with auth enabled, pointed at a caller-
    /// supplied JWKS cache (typically pre-seeded with a hand-minted pubkey).
    pub fn new_with_auth(
        meta: Arc<dyn MetadataStore>,
        blobs: Arc<dyn BlobStore>,
        configs: Arc<dyn ConfigStore>,
        listen_addr: String,
        entra: EntraConfig,
        jwks: Arc<JwksCache>,
    ) -> Arc<Self> {
        let auth = AuthState {
            mode: AuthMode::Enabled,
            entra: Some(entra),
            jwks,
        };
        Self::new(meta, blobs, configs, listen_addr, auth)
    }
}

/// Best-effort hostname lookup. Uses `HOSTNAME` / `COMPUTERNAME` env vars to
/// avoid pulling in a platform-specific crate for a debug-only field.
fn detect_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
