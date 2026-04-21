// api-server library root.
//
// Exposes the Axum router builder so integration tests can drive the server
// in-process without binding to a real port. The `cmtraceopen-api` binary
// in `src/main.rs` is a thin runtime wrapper around this library.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod extract;
pub mod routes;
pub mod state;
pub mod storage;

use std::sync::Arc;

use axum::Router;

use crate::state::AppState;

/// Build the Axum router with all routes attached.
///
/// This is the composition root — future modules (auth middleware, CORS,
/// tracing layers) plug in here. Takes a prebuilt [`AppState`] so integration
/// tests can inject a tempdir + in-memory SQLite while `main.rs` builds the
/// real one from env.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::ingest::router(state.clone()))
        .merge(routes::devices::router(state.clone()))
        .merge(routes::sessions::router(state))
}
