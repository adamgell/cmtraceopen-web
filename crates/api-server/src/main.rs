use std::process::ExitCode;
use std::sync::Arc;

use api_server::config::Config;
use api_server::router;
use api_server::state::{AppState, CorsConfig};
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
        version = env!("CARGO_PKG_VERSION"),
        "starting cmtraceopen-api"
    );

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

    // AppState is constructed here so `started_at` reflects the real
    // process start (before we block in `bind`). Cloned by reference into
    // the router and the request-counter middleware.
    let cors = CorsConfig {
        allowed_origins: config.allowed_origins.clone(),
        allow_credentials: config.allow_credentials,
    };
    let state = AppState::with_cors(meta, blobs, config.listen_addr.to_string(), cors);

    let app = router(state).layer(TraceLayer::new_for_http());

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
