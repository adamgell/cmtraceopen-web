use std::process::ExitCode;
use std::sync::Arc;

use api_server::auth::{AuthMode, AuthState, JwksCache};
use api_server::config::Config;
use api_server::router;
use api_server::state::{AppState, CorsConfig, MtlsRuntimeConfig};
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

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

    let blobs = match LocalFsBlobStore::new(&config.data_dir).await {
        Ok(b) => Arc::new(b),
        Err(err) => {
            eprintln!("fatal: failed to open blob store at {:?}: {err}", config.data_dir);
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
