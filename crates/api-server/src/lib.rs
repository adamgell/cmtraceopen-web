// api-server library root.
//
// Exposes the Axum router builder so integration tests can drive the server
// in-process without binding to a real port. The `cmtraceopen-api` binary
// in `src/main.rs` is a thin runtime wrapper around this library.

#![forbid(unsafe_code)]

pub mod auth;
pub mod config;
pub mod error;
pub mod extract;
pub mod pipeline;
pub mod routes;
pub mod state;
pub mod storage;

#[cfg(feature = "mtls")]
pub mod tls;

use std::sync::Arc;

use axum::http::{
    header::{AUTHORIZATION, CONTENT_TYPE},
    HeaderName, HeaderValue, Method,
};
use axum::{middleware, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use state::AppState;

/// Custom device-identity header surfaced by the temporary ingest auth model.
/// Kept in sync with the header name used by the Axum extractors in
/// `routes::ingest` — see `extract::DeviceId`.
const X_DEVICE_ID: HeaderName = HeaderName::from_static("x-device-id");

/// Build the Axum router with all routes attached.
///
/// This is the composition root — future modules (auth middleware, tracing
/// layers) plug in here. Takes a prebuilt [`AppState`] so integration tests
/// can inject a tempdir + in-memory SQLite while `main.rs` builds the real
/// one from env.
///
/// The shared [`AppState`] is threaded into the `/` status page (for
/// read-out) and the request-counter middleware (for bumping on each hit),
/// in addition to the ingest / devices / sessions sub-routers that consume
/// the storage handles.
///
/// ## Layer ordering
///
/// The CORS layer is attached **outermost** so browser preflight `OPTIONS`
/// requests are answered before they reach any auth / counter middleware.
/// Preflight responses must not depend on authentication state or they'll
/// blow the request before the real verb ever runs.
pub fn router(state: Arc<AppState>) -> Router {
    let cors = build_cors_layer(&state.cors);

    Router::new()
        .merge(routes::status::router(state.clone()))
        .merge(routes::health::router())
        .merge(routes::ingest::router(state.clone()))
        .merge(routes::devices::router(state.clone()))
        .merge(routes::sessions::router(state.clone()))
        .merge(routes::files::router(state.clone()))
        .merge(routes::entries::router(state.clone()))
        .merge(routes::admin::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state,
            routes::status::request_counter_middleware,
        ))
        .layer(cors)
}

/// Build the CORS layer from the runtime config.
///
/// Design:
/// - Uses [`AllowOrigin::list`] with exact origins parsed from the config's
///   `allowed_origins`. An empty list means the layer rejects all
///   cross-origin traffic (fail closed).
/// - Invalid origin strings are silently dropped — we log nothing here to
///   keep `router()` free of side effects, but `main.rs` validates origins
///   at startup via `Config::from_env` (future work: reject invalid origins
///   loudly).
/// - `allow_methods` covers every verb the current route table speaks.
///   Adding a new verb downstream will need this list updated.
/// - `allow_headers` is the minimum needed for the JSON API + ingest auth
///   header; CORS-safelisted headers (Accept, Accept-Language, etc.) don't
///   need to be listed.
fn build_cors_layer(cfg: &state::CorsConfig) -> CorsLayer {
    let parsed_origins: Vec<HeaderValue> = cfg
        .allowed_origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed_origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION, X_DEVICE_ID])
        .allow_credentials(cfg.allow_credentials)
}
