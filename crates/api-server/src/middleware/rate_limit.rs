//! Per-device and per-IP rate-limit middleware.
//!
//! Three Axum middleware functions guard two route groups:
//!
//! | Middleware                   | Applied to           | Key           | Window |
//! |------------------------------|----------------------|---------------|--------|
//! | [`device_ingest_middleware`] | `/v1/ingest/*`       | Device ID     | 1 h    |
//! | [`ip_ingest_middleware`]     | `/v1/ingest/*`       | Source IP     | 1 min  |
//! | [`ip_query_middleware`]      | query routes         | Source IP     | 1 min  |
//!
//! ## 429 response shape
//!
//! ```json
//! { "error": "rate_limit_exceeded", "message": "[device] ingest rate limit exceeded …" }
//! ```
//! Plus `Retry-After: <seconds>` (minimum 1 s, capped at the window size).
//!
//! ## Source-IP extraction
//!
//! The middleware reads `X-Forwarded-For` (first hop) and falls back to
//! `X-Real-Ip`. In production the Azure Application Gateway should be
//! configured to *overwrite* `X-Forwarded-For` so clients cannot spoof it.
//! When neither header is present the request is counted under the sentinel
//! key `"__unknown__"`; the limiter still fires at the configured threshold,
//! which provides a safe default for unproxied traffic.
//!
//! ## Metrics
//!
//! Every rejected request increments:
//! ```text
//! cmtrace_rate_limit_rejected_total{scope="device|ip", route="<template>"}
//! ```

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use common_wire::ErrorBody;

use crate::extract::DEVICE_ID_HEADER;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Middleware functions
// ---------------------------------------------------------------------------

/// Axum middleware: per-device-ID rate limit on bundle-ingest routes.
///
/// Reads the device identity from the `X-Device-Id` header (legacy path) or
/// from a future cert-identity extension. Requests that exceed the hourly
/// limit return 429 + `Retry-After`.
pub async fn device_ingest_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let limiter = match &state.rate_limit.device_ingest {
        Some(l) => l,
        None => return next.run(req).await,
    };

    let device_id = req
        .headers()
        .get(DEVICE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("__unknown__")
        .to_string();

    if let Err(retry_after) = limiter.check(&device_id) {
        let route = route_label(&req);
        metrics::counter!(
            "cmtrace_rate_limit_rejected_total",
            "scope" => "device",
            "route" => route,
        )
        .increment(1);
        return too_many_requests(
            retry_after.as_secs().max(1),
            "device",
            "ingest rate limit exceeded for this device; check Retry-After and reduce upload frequency",
        );
    }

    next.run(req).await
}

/// Axum middleware: per-source-IP rate limit on `/v1/ingest/*` routes.
///
/// Backstop that fires when a single host is cycling many device IDs or
/// sending an unexpected flood of requests. Returns 429 + `Retry-After`.
pub async fn ip_ingest_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let limiter = match &state.rate_limit.ip_ingest {
        Some(l) => l,
        None => return next.run(req).await,
    };

    let ip = extract_ip(&req);
    if let Err(retry_after) = limiter.check(&ip) {
        let route = route_label(&req);
        metrics::counter!(
            "cmtrace_rate_limit_rejected_total",
            "scope" => "ip",
            "route" => route,
        )
        .increment(1);
        return too_many_requests(
            retry_after.as_secs().max(1),
            "ip",
            "ingest rate limit exceeded for this source address; check Retry-After",
        );
    }

    next.run(req).await
}

/// Axum middleware: per-source-IP rate limit on query routes
/// (`/v1/devices`, sessions, files, entries).
///
/// The lower default ceiling (60 req/min) is well above any legitimate
/// operator-UI refresh cadence. Returns 429 + `Retry-After`.
pub async fn ip_query_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let limiter = match &state.rate_limit.ip_query {
        Some(l) => l,
        None => return next.run(req).await,
    };

    let ip = extract_ip(&req);
    if let Err(retry_after) = limiter.check(&ip) {
        let route = route_label(&req);
        metrics::counter!(
            "cmtrace_rate_limit_rejected_total",
            "scope" => "ip",
            "route" => route,
        )
        .increment(1);
        return too_many_requests(
            retry_after.as_secs().max(1),
            "ip",
            "query rate limit exceeded for this source address; check Retry-After",
        );
    }

    next.run(req).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `429 Too Many Requests` response with a `Retry-After` header and
/// a JSON hint body. The hint identifies *which* scope triggered the limit
/// (device vs IP) without leaking other devices' counts or bucket states.
fn too_many_requests(retry_after_secs: u64, scope: &str, hint: &str) -> Response {
    let body = ErrorBody {
        error: "rate_limit_exceeded".to_string(),
        message: format!("[{scope}] {hint}"),
    };
    let retry_val = HeaderValue::from_str(&retry_after_secs.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("60"));
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    resp.headers_mut().insert("retry-after", retry_val);
    resp
}

/// Extract the client IP from `X-Forwarded-For` (leftmost entry) or
/// `X-Real-Ip`. Falls back to `"__unknown__"` when neither header is present.
///
/// In production the AppGW WAF should overwrite `X-Forwarded-For` so this
/// value is trusted. In local dev or test it defaults to the loopback
/// sentinel, which counts against the same bucket as all other unidentified
/// local callers.
fn extract_ip(req: &Request<Body>) -> String {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            req.headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "__unknown__".to_string())
}

/// Return a short route label for the metrics tag. Uses the `MatchedPath`
/// extension if available (it is when the middleware is attached to a
/// sub-router after route matching); falls back to `"unknown"`.
fn route_label(req: &Request<Body>) -> String {
    req.extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
