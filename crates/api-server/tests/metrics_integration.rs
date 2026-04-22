//! Integration tests for the Prometheus `/metrics` endpoint.
//!
//! Boots the real router on an ephemeral loopback port (same harness as the
//! ingest / cors integration tests), then drives requests through the
//! middleware so the request-counter / latency-histogram fire, and finally
//! scrapes `/metrics` to assert the exposition format + metric names.
//!
//! Note: the metrics-rs Prometheus recorder is process-global. Other
//! integration tests in this binary may have already pushed samples through
//! the same registry — we therefore assert on the *presence* of metric
//! names rather than absolute counter values.

use std::sync::Arc;

use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use tempfile::TempDir;
use tokio::net::TcpListener;

struct TestServer {
    base: String,
    _tmp: TempDir,
}

async fn start_server() -> TestServer {
    let tmp = TempDir::new().expect("tempdir");
    let blobs = Arc::new(
        LocalFsBlobStore::new(tmp.path())
            .await
            .expect("blob store"),
    );
    let meta = Arc::new(
        SqliteMetadataStore::connect(":memory:")
            .await
            .expect("sqlite"),
    );
    let state = AppState::new_auth_disabled(meta.clone(), blobs, meta, "127.0.0.1:0".to_string());
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    TestServer { base, _tmp: tmp }
}

/// `GET /metrics` returns 200 with the Prometheus text-exposition
/// content-type and at least the request-counter metric in the body. We
/// drive a few `/healthz` hits first so the counter is guaranteed to have
/// fired regardless of test ordering inside the binary.
#[tokio::test]
async fn metrics_endpoint_returns_prometheus_exposition() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    // Warm the per-route counter — at least one matched route hit so the
    // path label is populated. /healthz is the cheapest probe available.
    for _ in 0..3 {
        let _ = client.get(format!("{}/healthz", server.base)).send().await;
    }

    let resp = client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .expect("metrics scrape");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Per the OpenMetrics / Prometheus 0.0.4 spec, scrapers expect the
    // version + charset bits in the content-type. tower-http and the
    // Prometheus servers themselves both check for them.
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .expect("content-type header");
    assert!(
        ct.contains("text/plain") && ct.contains("version=0.0.4"),
        "unexpected content-type: {ct}"
    );

    let body = resp.text().await.expect("body");

    // The request-counter metric must be present and labeled with the
    // matched-path template (not the raw URI). Using a substring assert
    // (rather than a strict equality on a sample line) keeps the test
    // resilient to other tests in the same binary having pushed samples
    // through the global recorder.
    assert!(
        body.contains("cmtrace_http_requests_total"),
        "missing cmtrace_http_requests_total in body:\n{body}"
    );
    assert!(
        body.contains("path=\"/healthz\""),
        "missing path=\"/healthz\" label in body:\n{body}"
    );

    // Exposition lines start with `# HELP` / `# TYPE` for documented
    // metrics. The describe_metrics() call in main.rs runs from
    // install_metrics_recorder, so we should see the HELP line for the
    // request counter too.
    //
    // (Tests don't go through main.rs so describe lines may be absent —
    // assert only that the metric line itself is well-formed.)
    let has_metric_line = body
        .lines()
        .any(|l| l.starts_with("cmtrace_http_requests_total{") && l.ends_with(|c: char| c.is_ascii_digit() || c == '0'));
    assert!(
        has_metric_line,
        "no concrete cmtrace_http_requests_total sample line in body:\n{body}"
    );
}

/// `/metrics` is available without authentication — Prometheus scrapers
/// don't speak Bearer tokens by default. Asserts there's no 401 / 403
/// in the auth-disabled path; the production lockdown happens at the
/// network layer (firewall to the scraper subnet only).
#[tokio::test]
async fn metrics_endpoint_is_unauthenticated() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .expect("metrics scrape");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}
