//! `GET /metrics` — Prometheus text-exposition endpoint.
//!
//! Returns the metrics-rs Prometheus recorder snapshot in the standard
//! 0.0.4 text format (`Content-Type: text/plain; version=0.0.4; charset=utf-8`).
//! Prometheus servers and Grafana Agent scrape this format natively; default
//! recommended scrape interval is 15s.
//!
//! ## Auth
//!
//! Intentionally unauthenticated — Prometheus scrapers don't speak Bearer
//! tokens by default and the operator-bearer auth on this server is geared
//! at human-driven JSON queries. In a real deployment, lock `/metrics` down
//! at the network layer (firewall / NetworkPolicy to the Prometheus pod
//! only) or expose it on a separate listener bound to a private interface.
//!
//! ## Why a separate route from `/healthz` and `/readyz`
//!
//! Liveness / readiness probes serve a different consumer (the orchestrator)
//! and must stay cheap + side-effect-free. `/metrics` renders the entire
//! recorder snapshot which can grow with cardinality, so we keep it on its
//! own path so probe latency doesn't drift as more metrics are added.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::state::AppState;

/// Handler: render the current Prometheus snapshot.
///
/// `PrometheusHandle::render()` walks the recorder's internal registry and
/// formats every counter / gauge / histogram into the text-exposition format
/// in one allocation. Called on every scrape (every 15s by default), so the
/// implementation stays allocation-light by reusing the formatter.
async fn metrics_handler(State(state): State<Arc<AppState>>) -> Response {
    let body = state.metrics.render();
    let mut resp = (StatusCode::OK, body).into_response();
    // The Prometheus exposition spec is precise about the version + charset
    // bits — older scrapers won't accept the response without them.
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    resp
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use crate::state::install_metrics_recorder;

    /// Sanity-check the metrics-rs round-trip: incrementing a counter via
    /// the macro must show up in the handle's rendered text. This is a
    /// process-global recorder so we use a uniquely-named counter to avoid
    /// colliding with samples emitted by other tests in the same binary.
    #[test]
    fn counter_increment_renders_in_handle_output() {
        let handle = install_metrics_recorder();

        // Counter name picked to be unique across the test binary so the
        // assertion is hermetic regardless of test order.
        let name = "cmtrace_test_metric_route_counter_total";
        metrics::counter!(name).increment(7);

        let rendered = handle.render();
        assert!(
            rendered.contains(name),
            "rendered output missing {name}:\n{rendered}"
        );
        // The sample line should include our incremented value. We don't
        // assert exact equality because other invocations from the same
        // test binary may have bumped it further.
        let line = rendered
            .lines()
            .find(|l| l.starts_with(name))
            .expect("counter sample line missing");
        let value: u64 = line
            .split_whitespace()
            .next_back()
            .and_then(|v| v.parse().ok())
            .expect("counter line tail must be a u64");
        assert!(value >= 7, "counter value {value} < expected ≥7");
    }
}
