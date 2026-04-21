use axum::{routing::get, Json, Router};
use common_wire::HealthResponse;

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        service: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Readiness is identical to liveness while there are no external deps wired
/// up. Once the server connects to Postgres / object store, `/readyz` will do
/// a real dependency probe and `/healthz` stays shallow.
async fn readyz() -> Json<HealthResponse> {
    healthz().await
}

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
}
