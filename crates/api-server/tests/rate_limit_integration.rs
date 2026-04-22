//! Integration tests for the per-device and per-IP rate limiting middleware.
//!
//! Each test spins up the real Axum router on an ephemeral loopback port with
//! deliberately low rate-limit thresholds so we can exhaust the window in a
//! handful of requests without sleeping for a full hour.
//!
//! ## What is tested
//!
//! * A device that exceeds its per-hour ingest limit receives 429s; other
//!   devices with separate identities continue to succeed.
//! * The 429 response includes a `Retry-After` header (≥ 1 s).
//! * The 429 body carries `"error": "rate_limit_exceeded"` and a `[device]`
//!   or `[ip]` scope hint.
//! * Setting a limit to `0` disables that scope (no 429 emitted).
//! * The per-IP ingest and per-IP query limiters use independent buckets so
//!   exhausting one doesn't affect the other.

use std::sync::Arc;
use std::time::Duration;

use api_server::router;
use api_server::config::RateLimitConfig;
use api_server::state::{AppState, RateLimitState};
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use common_wire::ingest::{BundleInitRequest, content_kind};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct TestServer {
    base: String,
    _tmp: TempDir,
}

/// Boot a server with the supplied rate-limit config. Returns the base URL
/// and the tempdir guard.
async fn start_server_with_rate_limit(cfg: RateLimitConfig) -> TestServer {
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
    let rate_limit = Arc::new(RateLimitState::from_config(&cfg));
    let state =
        AppState::new_auth_disabled_with_rate_limit(meta, blobs, "127.0.0.1:0".to_string(), rate_limit);
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    TestServer { base, _tmp: tmp }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// POST /v1/ingest/bundles for a given device and return the HTTP status.
async fn post_ingest_init(client: &reqwest::Client, base: &str, device_id: &str) -> u16 {
    let payload = b"test";
    client
        .post(format!("{base}/v1/ingest/bundles"))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id: Uuid::now_v7(),
            device_hint: None,
            sha256: sha256_hex(payload),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .expect("request")
        .status()
        .as_u16()
}

/// Returns true if the status code represents a successful ingest response
/// (200 OK or 201 Created).
fn is_ingest_ok(status: u16) -> bool {
    status == 200 || status == 201
}

/// GET /v1/devices and return the HTTP status.
#[allow(dead_code)]
async fn get_devices(client: &reqwest::Client, base: &str) -> u16 {
    client
        .get(format!("{base}/v1/devices"))
        .send()
        .await
        .expect("request")
        .status()
        .as_u16()
}

// ---------------------------------------------------------------------------
// Per-device ingest limit
// ---------------------------------------------------------------------------

/// A device that fires more than `limit` requests in the window should start
/// receiving 429s. Other devices with different IDs are unaffected.
#[tokio::test]
async fn per_device_limit_triggers_429_and_isolates_other_devices() {
    // Allow 2 ingest calls per "hour" (window is still 1 h in real time, but
    // at this tiny limit we exhaust it in 3 calls).
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 2,
        ingest_per_ip_minute: 0, // disabled — don't interfere
        query_per_ip_minute: 0,
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();

    // First two requests from device A — should succeed.
    assert!(
        is_ingest_ok(post_ingest_init(&client, &server.base, "device-A").await),
        "first request should succeed"
    );
    assert!(
        is_ingest_ok(post_ingest_init(&client, &server.base, "device-A").await),
        "second request should succeed"
    );

    // Third request from device A — limit exceeded.
    let status = post_ingest_init(&client, &server.base, "device-A").await;
    assert_eq!(status, 429, "third request should be rate-limited");

    // Device B (different identity) should still succeed.
    assert!(
        is_ingest_ok(post_ingest_init(&client, &server.base, "device-B").await),
        "device-B should be unaffected by device-A's limit"
    );
}

/// The 429 response from the device limiter must include a `Retry-After`
/// header (≥ 1 s) and a JSON body with the correct error code + scope hint.
#[tokio::test]
async fn per_device_429_includes_retry_after_and_scope_hint() {
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 1,
        ingest_per_ip_minute: 0,
        query_per_ip_minute: 0,
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let payload = b"test";

    // Consume the single allowed slot.
    client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", "limited-device")
        .json(&BundleInitRequest {
            bundle_id: Uuid::now_v7(),
            device_hint: None,
            sha256: sha256_hex(payload),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .expect("first")
        .error_for_status()
        .expect("first should succeed");

    // Next call should be 429.
    let resp = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", "limited-device")
        .json(&BundleInitRequest {
            bundle_id: Uuid::now_v7(),
            device_hint: None,
            sha256: sha256_hex(payload),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .expect("second");

    assert_eq!(resp.status().as_u16(), 429);

    // Retry-After header present and ≥ 1 s.
    let retry_after: u64 = resp
        .headers()
        .get("retry-after")
        .expect("Retry-After header missing")
        .to_str()
        .expect("Retry-After header not ASCII")
        .parse()
        .expect("Retry-After header not an integer");
    assert!(retry_after >= 1, "Retry-After should be ≥ 1 s, got {retry_after}");

    // Body includes the scope hint.
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["error"], "rate_limit_exceeded");
    let msg = body["message"].as_str().expect("message field");
    assert!(
        msg.contains("[device]"),
        "message should identify the device scope; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Disabled limits
// ---------------------------------------------------------------------------

/// Setting `ingest_per_device_hour = 0` disables the per-device limiter; no
/// 429 should be emitted regardless of how many requests a device sends.
#[tokio::test]
async fn zero_limit_disables_device_rate_limiting() {
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 0, // disabled
        ingest_per_ip_minute: 0,
        query_per_ip_minute: 0,
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();

    // Fire 5 requests — all should succeed (or fail for real business
    // reasons, but never 429).
    for i in 0..5u8 {
        let status = post_ingest_init(&client, &server.base, "flood-device").await;
        assert_ne!(
            status, 429,
            "request {i} should not be rate-limited when limit = 0"
        );
    }
}

// ---------------------------------------------------------------------------
// Per-IP ingest limit
// ---------------------------------------------------------------------------

/// When the per-IP ingest limit fires, the 429 body identifies the "ip" scope.
#[tokio::test]
async fn per_ip_ingest_limit_triggers_429_with_ip_scope() {
    // Use X-Forwarded-For so all requests look like they come from the same IP.
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 0, // don't interfere
        ingest_per_ip_minute: 2,
        query_per_ip_minute: 0,
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let payload = b"test";
    let fake_ip = "10.0.0.1";

    let make_req = || {
        client
            .post(format!("{}/v1/ingest/bundles", server.base))
            .header("x-device-id", &format!("dev-{}", Uuid::now_v7()))
            .header("x-forwarded-for", fake_ip)
            .json(&BundleInitRequest {
                bundle_id: Uuid::now_v7(),
                device_hint: None,
                sha256: sha256_hex(payload),
                size_bytes: payload.len() as u64,
                content_kind: content_kind::EVIDENCE_ZIP.into(),
            })
    };

    // First two succeed.
    assert!(is_ingest_ok(make_req().send().await.expect("r1").status().as_u16()));
    assert!(is_ingest_ok(make_req().send().await.expect("r2").status().as_u16()));

    // Third triggers the IP limit.
    let resp = make_req().send().await.expect("r3");
    assert_eq!(resp.status().as_u16(), 429);

    let body: serde_json::Value = resp.json().await.expect("JSON body");
    let msg = body["message"].as_str().expect("message field");
    assert!(
        msg.contains("[ip]"),
        "message should identify the ip scope; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Per-IP query limit
// ---------------------------------------------------------------------------

/// The per-IP query limiter guards /v1/devices and fires independently of
/// the ingest limiters.
#[tokio::test]
async fn per_ip_query_limit_triggers_429() {
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 0,
        ingest_per_ip_minute: 0,
        query_per_ip_minute: 2,
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();

    // Use a specific forwarded IP so all requests are counted together.
    let make_req = || {
        client
            .get(format!("{}/v1/devices", server.base))
            .header("x-forwarded-for", "10.0.0.2")
    };

    // First two succeed (auth is disabled so we get through to the handler).
    // The handler may return 200 or an auth error but should not be 429.
    let s1 = make_req().send().await.expect("r1").status().as_u16();
    assert_ne!(s1, 429, "first query request should not be rate-limited");

    let s2 = make_req().send().await.expect("r2").status().as_u16();
    assert_ne!(s2, 429, "second query request should not be rate-limited");

    // Third triggers the query IP limit.
    let resp = make_req().send().await.expect("r3");
    assert_eq!(resp.status().as_u16(), 429);

    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["error"], "rate_limit_exceeded");
    let msg = body["message"].as_str().expect("message field");
    assert!(msg.contains("[ip]"), "got: {msg}");
}

/// Query limits and ingest limits are independent — exhausting the query
/// bucket does not affect ingest routes.
#[tokio::test]
async fn query_limit_does_not_affect_ingest() {
    let server = start_server_with_rate_limit(RateLimitConfig {
        ingest_per_device_hour: 0, // unlimited ingest
        ingest_per_ip_minute: 0,
        query_per_ip_minute: 1, // tight query limit
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::new();
    let ip = "10.0.0.3";

    // Exhaust the query limit.
    client
        .get(format!("{}/v1/devices", server.base))
        .header("x-forwarded-for", ip)
        .send()
        .await
        .expect("query 1");

    let q2 = client
        .get(format!("{}/v1/devices", server.base))
        .header("x-forwarded-for", ip)
        .send()
        .await
        .expect("query 2");
    assert_eq!(q2.status().as_u16(), 429, "second query should be limited");

    // Ingest should still work from the same IP.
    let payload = b"test";
    let ingest_status = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", "dev-isolated")
        .header("x-forwarded-for", ip)
        .json(&BundleInitRequest {
            bundle_id: Uuid::now_v7(),
            device_hint: None,
            sha256: sha256_hex(payload),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .expect("ingest after query limit")
        .status()
        .as_u16();
    assert_ne!(
        ingest_status,
        429,
        "ingest should be unaffected by the query IP limit"
    );
}

// ---------------------------------------------------------------------------
// Window reset (wall-clock, uses tokio::time::pause)
// ---------------------------------------------------------------------------

/// After a very short window elapses, the counter resets and requests succeed
/// again. Uses a 1-second window to keep the test fast.
#[tokio::test]
async fn rate_limit_window_resets_after_duration() {
    // Build state directly with a 1-second window instead of going through
    // RateLimitConfig (which hardcodes hours/minutes).
    use api_server::state::RateLimiter;

    let limiter = RateLimiter::new(1, Duration::from_millis(200));

    // First call succeeds.
    assert!(limiter.check("key").is_ok());
    // Second call is rejected.
    assert!(limiter.check("key").is_err());

    // Wait for the window to expire.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Third call (new window) succeeds again.
    assert!(
        limiter.check("key").is_ok(),
        "counter should reset after the window expires"
    );
}
