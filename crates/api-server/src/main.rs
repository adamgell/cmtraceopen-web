use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use api_server::auth::{AuthMode, AuthState, JwksCache};
#[cfg(feature = "crl")]
use api_server::auth::CrlCache;
use api_server::config::Config;
use api_server::pipeline::retention;
use api_server::router;
use api_server::state::{
    install_metrics_recorder, AppState, CorsConfig, MtlsRuntimeConfig, RateLimitState,
};
use api_server::storage::{build_blob_store, ConfigStore, SqliteMetadataStore};
// build_metadata_store factory is intentionally NOT wired into main.rs at the
// moment: the Postgres backend (PR #77) ships its module + migrations but
// main.rs still constructs a SqliteMetadataStore directly so the audit_store /
// configs trait-object coercions stay simple. Wiring the factory up is a
// follow-up; tracked in the PR body.
#[allow(unused_imports)]
use api_server::storage::build_metadata_store;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Guard so the rustls crypto provider is installed exactly once. `rustls`
/// permits a single global provider per process; calling `install_default()`
/// twice returns `Err`. Future code paths (Wave 3 mTLS termination) will
/// also need the provider, so install it here at the top-level binary entry
/// point and let downstream code assume it's already present.
static CRYPTO_PROVIDER: OnceLock<()> = OnceLock::new();

fn install_crypto_provider() {
    CRYPTO_PROVIDER.get_or_init(|| {
        // `install_default` returns Err if a provider is already set —
        // shouldn't happen because the OnceLock gates entry, but if a future
        // refactor adds an earlier install we'd rather log + continue than
        // panic on `expect`.
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            warn!("rustls default crypto provider was already installed; using existing");
        }
    });
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    install_crypto_provider();

    // Install the Prometheus recorder before any code paths that emit
    // metrics. `install_metrics_recorder` is idempotent (wrapped in a
    // `OnceLock`), so doing it here in addition to lazy init from
    // `AppState::new` is harmless — it just guarantees metric describes
    // happen before the first scrape.
    let _metrics_handle = install_metrics_recorder();
    describe_metrics();

    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("fatal: {err}");
            return ExitCode::from(2);
        }
    };

    info!(
        listen_addr = %config.listen_addr,
        data_dir = %config.data_dir.display(),
        database_url = %config.database_url,
        blob_backend = ?config.blob_backend,
        cors_origins = ?config.allowed_origins,
        cors_credentials = config.allow_credentials,
        tls_enabled = config.tls.enabled,
        mtls_required_on_ingest = config.tls.require_on_ingest,
        san_uri_scheme = %config.tls.expected_san_uri_scheme,
        peer_cert_header = ?config.tls.peer_cert_header,
        trusted_proxy_cidr = ?config.tls.trusted_proxy_cidr,
        version = env!("CARGO_PKG_VERSION"),
        "starting cmtraceopen-api"
    );

    // Warn when operator set CMTRACE_TLS_ENABLED=true alongside
    // CMTRACE_PEER_CERT_HEADER — the config layer already forces TLS off
    // in that case; the warning makes the override explicit in the log.
    if config.tls.peer_cert_header.is_some() && std::env::var("CMTRACE_TLS_ENABLED")
        .ok()
        .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(()),
            _ => None,
        })
        .is_some()
    {
        warn!(
            "CMTRACE_TLS_ENABLED=true and CMTRACE_PEER_CERT_HEADER are both set; \
             in-process TLS has been disabled because AppGW terminates TLS — \
             remove CMTRACE_TLS_ENABLED to silence this warning."
        );
    }

    // Warn loudly when CMTRACE_TLS_ENABLED is set but the binary was built
    // without the `mtls` feature (e.g. on a dev box with no cmake/NASM).
    // The config layer parses the var unconditionally; only the bring-up
    // path below decides whether to honor it.
    #[cfg(not(feature = "mtls"))]
    if config.tls.enabled {
        warn!(
            "CMTRACE_TLS_ENABLED=true but binary was built without the `mtls` \
             feature; falling back to plaintext HTTP. Rebuild with \
             `--features mtls` to enable TLS termination."
        );
    }

    // Pick the blob backend based on env. The factory hands back a
    // trait-object Arc so this line is the only place that knows whether
    // we're talking to local-FS, Azure, or (future) S3/GCS — adding a new
    // backend doesn't touch main.rs.
    let blobs = match build_blob_store(&config).await {
        Ok(b) => b,
        Err(err) => {
            eprintln!(
                "fatal: failed to initialize blob store ({:?}): {err}",
                config.blob_backend
            );
            return ExitCode::from(1);
        }
    };

    // Direct SqliteMetadataStore construction — the Postgres factory
    // (build_metadata_store) is available but not yet wired through main
    // because audit + ConfigStore threading needs the concrete type.
    // Follow-up: put audit_store + configs on the MetadataStore trait so
    // the factory handoff works transparently.
    let meta_store = match SqliteMetadataStore::connect(&config.sqlite_path).await {
        Ok(m) => Arc::new(m),
        Err(err) => {
            eprintln!(
                "fatal: failed to open metadata store ({}): {err}",
                config.sqlite_path
            );
            return ExitCode::from(1);
        }
    };

    // The audit store shares the same SQLite pool as `meta_store` — calling
    // `audit_store()` is a cheap Arc clone, not a new connection. Must
    // happen before the trait-object coercions below since `audit_store`
    // lives on the concrete `SqliteMetadataStore` type.
    let audit = Arc::new(meta_store.audit_store());

    let meta: Arc<dyn api_server::storage::MetadataStore> = meta_store.clone();
    let configs: Arc<dyn ConfigStore> = meta_store;

    // Build the auth state. In production the JWKS cache is pre-warmed on
    // startup so the first real request doesn't pay for the discovery-URI
    // round-trip; refresh failures here are logged but not fatal (the cache
    // will try again on the first request).
    if matches!(config.auth_mode, AuthMode::Disabled) {
        warn!(
            "CMTRACE_AUTH_MODE=disabled — operator-bearer auth BYPASSED. \
             DEV-ONLY: never deploy with this flag."
        );
    }
    let jwks = match config.entra.as_ref() {
        Some(entra) => {
            let cache = Arc::new(JwksCache::new(entra.jwks_uri.clone()));
            if matches!(config.auth_mode, AuthMode::Enabled) {
                if let Err(err) = cache.refresh().await {
                    warn!(%err, "initial JWKS prefetch failed; will retry on first request");
                }
            }
            cache
        }
        None => Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string())),
    };
    let auth_state = AuthState {
        mode: config.auth_mode,
        entra: config.entra.clone(),
        jwks,
    };

    // Spin up the retention sweeper. Lives for the life of the process —
    // detached `tokio::spawn` is fine because the loop has no shutdown
    // handshake (every state transition is a single transaction; killing
    // the task mid-sleep or mid-delete leaves the DB consistent). When
    // `CMTRACE_BUNDLE_TTL_DAYS=0` the loop still runs but no-ops on every
    // tick; see `retention::run_retention_loop` for the gate.
    //
    // The meta clone uses an explicit binding rather than passing
    // `Arc::clone(&meta)` inline because `Arc<SqliteMetadataStore>` →
    // `Arc<dyn MetadataStore>` needs the unsizing coercion to fire,
    // which only happens at a let binding with an explicit type
    // annotation (or when passed into a function whose parameter type
    // already nails the trait object).
    {
        // `Arc::clone(&meta)` would bind T = dyn MetadataStore from the LHS
        // and then choke on `&meta: &Arc<SqliteMetadataStore>`. Method-call
        // syntax binds T to the concrete type, then the let's type
        // annotation triggers the unsizing coercion to the trait object.
        let meta_for_retention: Arc<dyn api_server::storage::MetadataStore> = meta.clone();
        let blobs_for_retention = Arc::clone(&blobs);
        let cfg_for_retention = config.clone();
        tokio::spawn(async move {
            retention::run_retention_loop(
                cfg_for_retention,
                meta_for_retention,
                blobs_for_retention,
            )
            .await;
        });
    }

    // AppState is constructed here so `started_at` reflects the real
    // process start (before we block in `bind`). Cloned by reference into
    // the router and the request-counter middleware.
    let cors = CorsConfig {
        allowed_origins: config.allowed_origins.clone(),
        allow_credentials: config.allow_credentials,
    };

    // Load the CA bundle DER bytes for the header-cert path. This is only
    // needed when CMTRACE_PEER_CERT_HEADER is set; TlsConfig::from_env
    // already verified that CMTRACE_CLIENT_CA_BUNDLE is present in that
    // case, so the `expect` below is unreachable in practice.
    #[allow(unused_mut)]
    let mut trusted_ca_ders: Vec<Vec<u8>> = vec![];
    #[cfg(feature = "mtls")]
    if config.tls.peer_cert_header.is_some() {
        let ca_path = config
            .tls
            .client_ca_bundle
            .as_ref()
            .expect("TlsConfig::from_env ensures client_ca_bundle is set when peer_cert_header is set");
        match api_server::tls::load_ca_ders_from_pem(ca_path) {
            Ok(ders) => {
                info!(
                    count = ders.len(),
                    path = %ca_path.display(),
                    "CA bundle loaded for header-cert chain validation",
                );
                trusted_ca_ders = ders;
            }
            Err(err) => {
                eprintln!("fatal: failed to load CA bundle for peer-cert-header mode: {err}");
                return ExitCode::from(1);
            }
        }
    }

    let mtls = MtlsRuntimeConfig {
        require_on_ingest: config.tls.require_on_ingest,
        expected_san_uri_scheme: config.tls.expected_san_uri_scheme.clone(),
        peer_cert_header: config.tls.peer_cert_header.clone(),
        trusted_proxy_cidr: config.tls.trusted_proxy_cidr,
        trusted_ca_ders,
    };
    let rate_limit = std::sync::Arc::new(RateLimitState::from_config(&config.rate_limit));
    info!(
        ingest_per_device_hour = config.rate_limit.ingest_per_device_hour,
        ingest_per_ip_minute = config.rate_limit.ingest_per_ip_minute,
        query_per_ip_minute = config.rate_limit.query_per_ip_minute,
        trusted_proxy_cidrs = ?config.rate_limit.trusted_proxy_cidrs,
        "rate limiting configured (0 = disabled)",
    );

    // Spawn a background GC task that sweeps expired entries from every
    // active rate-limiter once per minute. This bounds the DashMap footprint
    // to the number of distinct keys seen in a single window rather than
    // growing indefinitely as new device IDs / IPs arrive over the life of
    // the process.
    {
        let rl = Arc::clone(&rate_limit);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let mut total_purged: usize = 0;
                if let Some(l) = &rl.device_ingest {
                    let before = l.len();
                    l.purge_expired();
                    total_purged += before.saturating_sub(l.len());
                }
                if let Some(l) = &rl.ip_ingest {
                    let before = l.len();
                    l.purge_expired();
                    total_purged += before.saturating_sub(l.len());
                }
                if let Some(l) = &rl.ip_query {
                    let before = l.len();
                    l.purge_expired();
                    total_purged += before.saturating_sub(l.len());
                }
                if total_purged > 0 {
                    tracing::debug!(purged = total_purged, "rate-limit GC: evicted expired entries");
                }
            }
        });
    }

    // Build the CRL cache (if the `crl` feature is on) and prime it with
    // an initial fetch before the listener binds. Refresh task continues
    // in the background for the life of the process.
    #[cfg(feature = "crl")]
    let crl_cache = if config.crl_urls.is_empty() {
        info!("CMTRACE_CRL_URLS empty; skipping CRL polling");
        None
    } else {
        let cache = Arc::new(CrlCache::new(
            config.crl_urls.clone(),
            std::time::Duration::from_secs(config.crl_refresh_secs),
            config.crl_fail_open,
        ));
        info!(
            urls = ?config.crl_urls,
            refresh_secs = config.crl_refresh_secs,
            fail_open = config.crl_fail_open,
            "starting CRL refresh task",
        );
        Arc::clone(&cache).start_refresh_task().await;
        if config.crl_fail_open {
            warn!(
                "CMTRACE_CRL_FAIL_OPEN=true — revoked certs MAY be accepted \
                 if every CRL fetch has failed since startup. Document the \
                 compensating control before deploying with this flag."
            );
        }
        Some(cache)
    };

    #[cfg(feature = "crl")]
    let state = AppState::with_cors_crl_and_audit(
        meta,
        blobs,
        configs,
        config.listen_addr.to_string(),
        auth_state,
        cors,
        mtls,
        crl_cache,
        rate_limit,
        audit,
    );
    #[cfg(not(feature = "crl"))]
    let state = AppState::full_with_audit(
        meta,
        blobs,
        configs,
        config.listen_addr.to_string(),
        auth_state,
        cors,
        mtls,
        rate_limit,
        audit,
    );

    let app = router(state).layer(TraceLayer::new_for_http());

    // Serve path: TLS-terminating axum-server when `tls_enabled` is true and
    // the `mtls` Cargo feature is on; plain `axum::serve` otherwise. The
    // TLS branch is feature-gated so dev boxes without cmake/NASM (and the
    // aws-lc-sys C build) can still build the binary and run it in plaintext
    // mode.
    #[cfg(feature = "mtls")]
    if config.tls.enabled {
        match api_server::tls::serve_tls(config.listen_addr, app, &config.tls).await {
            Ok(()) => {
                info!("cmtraceopen-api stopped cleanly (tls)");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("fatal: tls server error: {err}");
                return ExitCode::from(1);
            }
        }
    }

    let listener = match TcpListener::bind(config.listen_addr).await {
        Ok(l) => l,
        Err(err) => {
            eprintln!("fatal: failed to bind {}: {err}", config.listen_addr);
            return ExitCode::from(1);
        }
    };

    // Use `into_make_service_with_connect_info` so the rate-limit middleware
    // can read the real TCP peer address (`ConnectInfo<SocketAddr>`) and
    // bypass forwarded-header spoofing when the caller is not a trusted proxy.
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());

    if let Err(err) = serve.await {
        eprintln!("fatal: server error: {err}");
        return ExitCode::from(1);
    }

    info!("cmtraceopen-api stopped cleanly");
    ExitCode::SUCCESS
}

/// Register HELP / TYPE descriptions for every metric the server emits.
///
/// The metrics-rs facade stores these in the recorder so they show up in
/// the `/metrics` response alongside the samples. Prometheus itself will
/// scrape without descriptions, but Grafana's metric browser (and human
/// operators) rely on them to explain what each counter means. Keep the
/// list in sync with every `metrics::counter!()` / `histogram!()` /
/// `gauge!()` call site in the crate.
fn describe_metrics() {
    use metrics::{describe_counter, describe_gauge, describe_histogram, Unit};

    describe_counter!(
        "cmtrace_http_requests_total",
        "Total HTTP requests served, labeled by matched route template."
    );
    describe_histogram!(
        "cmtrace_http_request_duration_seconds",
        Unit::Seconds,
        "End-to-end HTTP request handler duration in seconds, labeled by matched route template."
    );
    describe_counter!(
        "cmtrace_ingest_bundles_initiated_total",
        "Bundles for which an ingest /init request was accepted."
    );
    describe_counter!(
        "cmtrace_ingest_bundles_finalized_total",
        "Bundle-finalize attempts, labeled by outcome: ok | partial | failed."
    );
    describe_counter!(
        "cmtrace_ingest_chunks_received_total",
        "Chunks successfully appended to staged uploads."
    );
    describe_counter!(
        "cmtrace_parse_worker_runs_total",
        "Background parse-worker runs, labeled by result: ok | partial | failed."
    );
    describe_histogram!(
        "cmtrace_parse_worker_duration_seconds",
        Unit::Seconds,
        "Wall-clock time spent in the background parse worker per session."
    );
    describe_gauge!(
        "cmtrace_db_connections_in_use",
        "Metadata-store pool connections currently checked out (size - idle)."
    );
    #[cfg(feature = "crl")]
    describe_counter!(
        "cmtrace_crl_revocations_total",
        "CRL revocation lookups in the DeviceIdentity extractor, labeled by \
         result: rejected (serial in CRL) | unknown_fail_open (cache cold, \
         allowed by crl_fail_open=true) | unknown_fail_closed (cache cold, \
         rejected with 503)."
    );
    describe_counter!(
        retention::M_SWEEPS,
        "Total bundle-retention sweep passes run since process start (one per CMTRACE_RETENTION_SCAN_INTERVAL_SECS tick)."
    );
    describe_counter!(
        retention::M_SESSIONS_DELETED,
        "Sessions hard-deleted (blob + metadata) by the retention sweeper."
    );
    describe_counter!(
        retention::M_BYTES_FREED,
        "Approximate blob bytes freed by the retention sweeper, summed from head_blob before delete."
    );
    describe_counter!(
        retention::M_ERRORS,
        "Retention-sweeper failures, labeled by stage: scan | blob | metadata."
    );
    describe_counter!(
        "cmtrace_peer_cert_source_total",
        "Device-identity extractions by source: header (AppGW cert header \
         accepted), header_invalid (AppGW cert header rejected — decode \
         failure or chain validation failure), tls (in-process mTLS), or \
         none (no cert / identity unavailable)."
    );
    describe_counter!(
        "cmtrace_rate_limit_rejected_total",
        "Requests rejected by the rate limiter, labeled by scope (device|ip) and route."
    );
}

fn init_tracing() {
    // JSON formatter so container logs feed straight into aggregators.
    // Override verbosity with RUST_LOG; default to info for our crates and
    // warn for noisy transitive libs (hyper, h2).
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("api_server=info,tower_http=info,axum=info,warn"));

    tracing_subscriber::registry()
        .with(fmt::layer().json().with_current_span(false))
        .with(filter)
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = signal::ctrl_c().await {
            warn!(%err, "failed to install ctrl-c handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => warn!(%err, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("received ctrl-c, shutting down"),
        _ = terminate => info!("received SIGTERM, shutting down"),
    }
}
