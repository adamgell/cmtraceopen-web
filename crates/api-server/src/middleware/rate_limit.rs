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
//! The middleware uses the TCP peer address (`ConnectInfo<SocketAddr>`) when
//! it is available (production path — `main.rs` uses
//! `into_make_service_with_connect_info`).  If the peer address falls inside
//! one of the configured `CMTRACE_TRUSTED_PROXY_CIDRS` CIDRs, the first value
//! in `X-Forwarded-For` (or `X-Real-Ip`) is used instead so the AppGW WAF
//! forwarded address is counted rather than the AppGW's own address.
//!
//! If `ConnectInfo` is absent (integration-test path that uses plain
//! `axum::serve`) the old header-first fallback behaviour is preserved so
//! tests can still simulate different source IPs via `X-Forwarded-For`.
//!
//! When trusted proxy CIDRs are empty (the default), the peer address is
//! **always** used and forwarded headers are completely ignored for IP-based
//! limiting — headers cannot be spoofed by the client.
//!
//! ## Device identity
//!
//! The device limiter key is resolved in the same priority order as the
//! [`crate::auth::DeviceIdentity`] extractor:
//!
//! 1. `DeviceIdentity` already stashed in request extensions (by a prior
//!    middleware layer).
//! 2. mTLS peer-cert SAN URI (`PeerCertChain` extension, `mtls` feature).
//! 3. `X-Device-Id` header (legacy transitional path).
//! 4. `"__unknown__"` sentinel.
//!
//! ## Metrics
//!
//! Every rejected request increments (only when the matched route template is
//! known — the `"unknown"` label is dropped to keep cardinality bounded):
//! ```text
//! cmtrace_rate_limit_rejected_total{scope="device|ip", route="<template>"}
//! ```

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
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
/// Resolves the device identity from (in order) a previously-stashed
/// `DeviceIdentity` extension, the mTLS cert SAN URI, or the `X-Device-Id`
/// header. Requests that exceed the hourly limit return 429 + `Retry-After`.
pub async fn device_ingest_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let limiter = match &state.rate_limit.device_ingest {
        Some(l) => l,
        None => return next.run(req).await,
    };

    let device_id = resolve_device_id(&req, &state);

    if let Err(retry_after) = limiter.check(&device_id) {
        if let Some(route) = route_label(&req) {
            metrics::counter!(
                "cmtrace_rate_limit_rejected_total",
                "scope" => "device",
                "route" => route,
            )
            .increment(1);
        }
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

    let ip = extract_ip(&req, &state.rate_limit.trusted_proxy_cidrs);
    if let Err(retry_after) = limiter.check(&ip) {
        if let Some(route) = route_label(&req) {
            metrics::counter!(
                "cmtrace_rate_limit_rejected_total",
                "scope" => "ip",
                "route" => route,
            )
            .increment(1);
        }
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

    let ip = extract_ip(&req, &state.rate_limit.trusted_proxy_cidrs);
    if let Err(retry_after) = limiter.check(&ip) {
        if let Some(route) = route_label(&req) {
            metrics::counter!(
                "cmtrace_rate_limit_rejected_total",
                "scope" => "ip",
                "route" => route,
            )
            .increment(1);
        }
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

/// Resolve the device key for rate limiting.
///
/// Priority order (mirrors [`crate::auth::DeviceIdentity`]):
/// 1. `DeviceIdentity` already stashed in request extensions.
/// 2. mTLS `PeerCertChain` SAN URI (requires `mtls` feature).
/// 3. `X-Device-Id` header (legacy transitional path).
/// 4. `"__unknown__"` sentinel.
fn resolve_device_id(req: &Request<Body>, state: &AppState) -> String {
    // 1. Already resolved by a prior middleware/extension?
    if let Some(id) = req.extensions().get::<crate::auth::DeviceIdentity>() {
        return id.device_id.clone();
    }

    // 2. mTLS peer-cert SAN URI (only compiled in with the `mtls` feature).
    #[cfg(feature = "mtls")]
    if let Some(chain) = req.extensions().get::<crate::tls::PeerCertChain>() {
        if let Some(leaf) = chain.leaf() {
            if let Some(parsed) =
                crate::auth::device_identity::extract_device_id_from_leaf(
                    leaf.as_ref(),
                    &state.mtls.expected_san_uri_scheme,
                )
            {
                return parsed;
            }
        }
    }

    // Suppress "unused variable" warning when the mtls feature is off.
    let _ = state;

    // 3. Legacy `X-Device-Id` header fallback.
    if let Some(id) = req
        .headers()
        .get(DEVICE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 256)
    {
        return id;
    }

    // 4. Sentinel.
    "__unknown__".to_string()
}

/// Extract the effective client IP to use as the rate-limit key.
///
/// ## Trust model
///
/// **If `ConnectInfo<SocketAddr>` is present** (production — server was
/// started with `into_make_service_with_connect_info`):
/// - Peer IP **not** in any configured trusted-proxy CIDR → use peer IP
///   directly. Forwarded headers are ignored; they cannot be spoofed.
/// - Peer IP **is** in a trusted-proxy CIDR (AppGW etc.) → read the
///   first token from `X-Forwarded-For`, then `X-Real-Ip`. The AppGW is
///   required to overwrite (not append) that header in the WAF rules.
///
/// **If `ConnectInfo` is absent** (integration-test path that uses plain
/// `axum::serve`):
/// - Fall back to the old header-first behaviour so tests can simulate
///   different source IPs via `X-Forwarded-For`.
///
/// When no useful IP is found the sentinel `"__unknown__"` is returned so
/// traffic still counts against *some* bucket.
fn extract_ip(req: &Request<Body>, trusted_cidrs: &[ipnet::IpNet]) -> String {
    // Try to get the real TCP peer address from ConnectInfo.
    if let Some(ConnectInfo(peer_addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        let peer_ip = peer_addr.ip();
        if trusted_cidrs.is_empty() || !cidr_contains(trusted_cidrs, peer_ip) {
            // Not from a trusted proxy — use peer IP directly.
            return peer_ip.to_string();
        }
        // Peer is a trusted proxy — honor its forwarded header.
        if let Some(forwarded) = forwarded_ip_from_headers(req) {
            return forwarded;
        }
        // Trusted proxy but no header set — fall through to peer IP.
        return peer_ip.to_string();
    }

    // No ConnectInfo (test path) — fall back to header-based extraction.
    forwarded_ip_from_headers(req).unwrap_or_else(|| "__unknown__".to_string())
}

/// Check whether `ip` is contained in any of the supplied CIDR ranges.
fn cidr_contains(cidrs: &[ipnet::IpNet], ip: IpAddr) -> bool {
    cidrs.iter().any(|cidr| cidr.contains(&ip))
}

/// Read the first value from `X-Forwarded-For`, then `X-Real-Ip`.
fn forwarded_ip_from_headers(req: &Request<Body>) -> Option<String> {
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
}

/// Return the matched-route template, or `None` when the route is unknown.
///
/// Unknown routes are excluded from the metric to keep cardinality bounded
/// (un-matched paths can be arbitrary attacker-controlled strings).
fn route_label(req: &Request<Body>) -> Option<String> {
    req.extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_string())
}
