// api-server library root.
//
// Exposes the Axum router builder so integration tests can drive the server
// in-process without binding to a real port. The `cmtraceopen-api` binary
// in `src/main.rs` is a thin runtime wrapper around this library.

#![forbid(unsafe_code)]

pub mod config;
pub mod routes;

use std::sync::Arc;

use axum::{middleware, Router};

pub use routes::status::AppState;

/// Build the Axum router with all routes attached.
///
/// The shared [`AppState`] is threaded into both the `/` status page (for
/// read-out) and the request-counter middleware (for bumping on each hit).
/// Future modules (ingest, devices, sessions, auth middleware, CORS, tracing
/// layers) plug in here — keep per-route state localized via `with_state`
/// so this composition root stays small.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::status::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state,
            routes::status::request_counter_middleware,
        ))
}
