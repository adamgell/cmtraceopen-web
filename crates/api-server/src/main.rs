use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use api_server::auth::{AuthMode, AuthState, JwksCache};
#[cfg(feature = "crl")]
use api_server::auth::CrlCache;
use api_server::config::Config;
use api_server::router;
use api_server::state::{install_metrics_recorder, AppState, CorsConfig, MtlsRuntimeConfig};
use api_server::storage::{build_blob_store, SqliteMetadataStore};
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
        sqlite_path = %config.sqlite_path,
        blob_backend = ?config.blob_backend,
        cors_origins = ?config.allowed_origins,
        cors_credentials = config.allow_credentials,
        tls_enabled = config.tls.enabled,
        mtls_required_on_ingest = config.tls.require_on_ingest,
        san_uri_scheme = %config.tls.expected_san_uri_scheme,
        version = env!("CARGO_PKG_VERSION"),
        "starting cmtraceopen-api"
    );

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

    let meta = match SqliteMetadataStore::connect(&config.sqlite_path).await {
        Ok(m) => Arc::new(m),
        Err(err) => {
            eprintln!("fatal: failed to open sqlite at {}: {err}", config.sqlite_path);
            return ExitCode::from(1);
        }
    };

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

    // AppState is constructed here so `started_at` reflects the real
    // process start (before we block in `bind`). Cloned by reference into
    // the router and the request-counter middleware.
    let cors = CorsConfig {
        allowed_origins: config.allowed_origins.clone(),
        allow_credentials: config.allow_credentials,
    };
    let mtls = MtlsRuntimeConfig {
        require_on_ingest: config.tls.require_on_ingest,
        expected_san_uri_scheme: config.tls.expected_san_uri_scheme.clone(),
    };

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
    let state = AppState::with_cors_and_crl(
        meta,
        blobs,
        config.listen_addr.to_string(),
        auth_state,
        cors,
        mtls,
        crl_cache,
    );
    #[cfg(not(feature = "crl"))]
    let state = AppState::full(
        meta,
        blobs,
        config.listen_addr.to_string(),
        auth_state,
        cors,
        mtls,
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

    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

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
