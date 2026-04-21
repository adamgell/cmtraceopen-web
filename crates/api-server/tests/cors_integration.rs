//! Integration tests for the CORS layer.
//!
//! These tests spin up the real Axum server on an ephemeral loopback port,
//! backed by an in-memory SQLite + tempdir blob store (same scaffolding as
//! `ingest_integration.rs`), and drive browser-style preflight `OPTIONS`
//! requests via reqwest to verify the CORS headers.
//!
//! We test both the allow path (an origin that matches `CMTRACE_CORS_ORIGINS`)
//! and the deny path (an origin that doesn't). The deny path is important:
//! the `tower-http` CORS layer MUST NOT add `Access-Control-Allow-Origin` for
//! a non-matching origin, because that's the header browsers use to decide
//! whether to deliver the response to JS.

use std::sync::Arc;

use api_server::router;
use api_server::state::{AppState, CorsConfig};
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use tempfile::TempDir;
use tokio::net::TcpListener;

struct TestServer {
    base: String,
    _tmp: TempDir,
}

/// Boot a server with the given CORS config. Mirrors the helper in
/// `ingest_integration.rs` but takes an explicit `CorsConfig` so each test
/// can declare the allowed origins it expects.
async fn start_server_with_cors(cors: CorsConfig) -> TestServer {
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
    let state = AppState::with_cors(meta, blobs, "127.0.0.1:0".to_string(), cors);
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    TestServer { base, _tmp: tmp }
}

/// Preflight from a declared allowed origin returns 2xx + the expected
/// `Access-Control-Allow-*` headers, including the X-Device-Id ingest header.
#[tokio::test]
async fn preflight_from_allowed_origin_is_accepted() {
    let cors = CorsConfig {
        allowed_origins: vec!["http://localhost:5173".to_string()],
        allow_credentials: false,
    };
    let server = start_server_with_cors(cors).await;
    let client = reqwest::Client::new();

    // OPTIONS to a real route — path doesn't matter for the preflight check,
    // the CORS layer short-circuits before routing, but using a live path
    // keeps the test honest if the layer ordering ever regresses.
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("{}/healthz", server.base))
        .header("Origin", "http://localhost:5173")
        .header("Access-Control-Request-Method", "GET")
        .header(
            "Access-Control-Request-Headers",
            "content-type,authorization,x-device-id",
        )
        .send()
        .await
        .expect("preflight send");

    // tower-http returns 200 for a successful preflight. (Some CORS impls
    // use 204 — accept either to stay resilient to minor upstream changes.)
    let status = resp.status();
    assert!(
        status == reqwest::StatusCode::OK || status == reqwest::StatusCode::NO_CONTENT,
        "expected 200 or 204 preflight status, got {status}"
    );

    let allow_origin = resp
        .headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .expect("missing Access-Control-Allow-Origin on allowed preflight");
    assert_eq!(allow_origin, "http://localhost:5173");

    let allow_methods = resp
        .headers()
        .get("access-control-allow-methods")
        .and_then(|v| v.to_str().ok())
        .expect("missing Access-Control-Allow-Methods on allowed preflight")
        .to_ascii_uppercase();
    assert!(
        allow_methods.contains("GET"),
        "Access-Control-Allow-Methods missing GET: {allow_methods}"
    );

    let allow_headers = resp
        .headers()
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .expect("missing Access-Control-Allow-Headers on allowed preflight")
        .to_ascii_lowercase();
    for needed in ["content-type", "authorization", "x-device-id"] {
        assert!(
            allow_headers.contains(needed),
            "Access-Control-Allow-Headers missing {needed}: {allow_headers}"
        );
    }
}

/// Preflight from an origin NOT in `CMTRACE_CORS_ORIGINS` must NOT receive
/// an `Access-Control-Allow-Origin` header — that's the signal browsers use
/// to block the response. tower-http's CORS layer simply omits the header,
/// which is the correct behavior.
#[tokio::test]
async fn preflight_from_disallowed_origin_gets_no_allow_origin_header() {
    let cors = CorsConfig {
        allowed_origins: vec!["http://localhost:5173".to_string()],
        allow_credentials: false,
    };
    let server = start_server_with_cors(cors).await;
    let client = reqwest::Client::new();

    let resp = client
        .request(reqwest::Method::OPTIONS, format!("{}/healthz", server.base))
        .header("Origin", "http://evil.example.com")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("preflight send");

    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "Access-Control-Allow-Origin should be absent for disallowed origin, \
         got: {:?}",
        resp.headers().get("access-control-allow-origin")
    );
}
