//! Integration tests for operator-bearer auth on query routes.
//!
//! Three scenarios are covered end-to-end:
//!   - Enabled, no `Authorization` header → 401 + `WWW-Authenticate: Bearer`.
//!   - Enabled, valid Entra-issued JWT (hand-minted against a test keypair,
//!     pubkey stashed in the `JwksCache`) → 200.
//!   - Disabled (dev bypass) → 200 without any header.
//!
//! Ingest routes stay unauthenticated, so no explicit test for them here —
//! the ingest_integration.rs suite continues to drive them without tokens
//! and would regress if the router accidentally gated ingest on auth.

use std::sync::Arc;

use api_server::auth::{EntraConfig, JwksCache};
use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use jwt_simple::prelude::{
    Claims, Deserialize, Duration as JwtDuration, RS256KeyPair, RSAKeyPairLike, Serialize,
};
use reqwest::StatusCode;
use tempfile::TempDir;
use tokio::net::TcpListener;

const TEST_TENANT: &str = "11111111-1111-1111-1111-111111111111";
const TEST_AUDIENCE: &str = "api://cmtraceopen-test";

#[derive(Serialize, Deserialize)]
struct TestClaims {
    scp: String,
    name: String,
    tid: String,
}

/// Variant of [`TestClaims`] for app-only / role-bearing tokens. Both `scp`
/// and `roles` are optional and surfaced as bare-`null` on absence so the
/// server-side struct's `#[serde(default)]` kicks in.
#[derive(Serialize, Deserialize)]
struct RoleClaims {
    #[serde(skip_serializing_if = "Option::is_none")]
    scp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    roles: Option<Vec<String>>,
    name: String,
    tid: String,
}

struct TestServer {
    base: String,
    _tmp: TempDir,
}

async fn start_server_auth_enabled(
    jwks: Arc<JwksCache>,
) -> TestServer {
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
    let entra = EntraConfig {
        tenant_id: TEST_TENANT.to_string(),
        audience: TEST_AUDIENCE.to_string(),
        jwks_uri: "http://127.0.0.1:1/unused".to_string(),
    };
    let state = AppState::new_with_auth(meta, blobs, "127.0.0.1:0".to_string(), entra, jwks);
    let app = router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer { base, _tmp: tmp }
}

async fn start_server_auth_disabled() -> TestServer {
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
    let state = AppState::new_auth_disabled(meta, blobs, "127.0.0.1:0".to_string());
    let app = router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer { base, _tmp: tmp }
}

#[tokio::test]
async fn devices_rejects_missing_bearer_when_enabled() {
    // A totally empty JwksCache is sufficient — the extractor short-circuits
    // on the missing-header path before it touches signing keys.
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    let server = start_server_auth_enabled(jwks).await;

    let resp = reqwest::Client::new()
        .get(format!("{}/v1/devices", server.base))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // RFC 6750: challenge with at least `Bearer` scheme so clients know the
    // realm + error category.
    let www = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate");
    let val = www.to_str().unwrap();
    assert!(val.starts_with("Bearer"), "want Bearer challenge, got {val}");
    assert!(val.contains("error=\"invalid_token\""), "got {val}");
}

#[tokio::test]
async fn devices_accepts_valid_bearer_when_enabled() {
    // Hand-mint an RS256 keypair, stash its pubkey in the JWKS cache under
    // a fixed kid, and sign a token whose claims line up with the expected
    // issuer / audience / scope. The extractor then validates against that
    // cache without ever touching the network.
    let kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test("test-kid", kp.public_key());

    let server = start_server_auth_enabled(jwks.clone()).await;

    let custom = TestClaims {
        scp: "CmtraceOpen.Query openid profile".to_string(),
        name: "Test Operator".to_string(),
        tid: TEST_TENANT.to_string(),
    };
    let issuer = format!("https://login.microsoftonline.com/{}/v2.0", TEST_TENANT);
    let claims = Claims::with_custom_claims(custom, JwtDuration::from_secs(300))
        .with_issuer(&issuer)
        .with_audience(TEST_AUDIENCE)
        .with_subject("operator@example.com");
    let kp_kid = kp.with_key_id("test-kid");
    let token = kp_kid.sign(claims).expect("sign");

    let resp = reqwest::Client::new()
        .get(format!("{}/v1/devices", server.base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("send");

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "body: {}",
        resp.text().await.unwrap_or_default()
    );
}

#[tokio::test]
async fn devices_allowed_without_bearer_when_disabled() {
    let server = start_server_auth_disabled().await;
    let resp = reqwest::Client::new()
        .get(format!("{}/v1/devices", server.base))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// RBAC: operator vs admin route gating
// ---------------------------------------------------------------------------

/// Hand-mint a token carrying the supplied `scp` + `roles` claim values.
/// Reused across the role-gating integration cases below.
fn mint_role_token(
    kp: &RS256KeyPair,
    kid: &str,
    scp: Option<&str>,
    roles: Option<Vec<String>>,
) -> String {
    let issuer = format!("https://login.microsoftonline.com/{}/v2.0", TEST_TENANT);
    let custom = RoleClaims {
        scp: scp.map(str::to_string),
        roles,
        name: "Test Operator".to_string(),
        tid: TEST_TENANT.to_string(),
    };
    let claims = Claims::with_custom_claims(custom, JwtDuration::from_secs(300))
        .with_issuer(&issuer)
        .with_audience(TEST_AUDIENCE)
        .with_subject("operator@example.com");
    let kp_kid = kp.clone().with_key_id(kid);
    kp_kid.sign(claims).expect("sign")
}

/// Operator-only token (delegated `CmtraceOpen.Query` scope) hitting an
/// admin-only route must 403 — admin role is mandatory and the principal
/// doesn't carry it.
#[tokio::test]
async fn operator_token_forbidden_on_admin_route() {
    let kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test("test-kid", kp.public_key());
    let server = start_server_auth_enabled(jwks).await;

    let token = mint_role_token(
        &kp,
        "test-kid",
        Some("CmtraceOpen.Query openid profile"),
        None,
    );

    let resp = reqwest::Client::new()
        .post(format!(
            "{}/v1/admin/devices/some-device/disable",
            server.base
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("send");

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "body: {}",
        resp.text().await.unwrap_or_default()
    );
}

/// Admin token hitting the admin route gets through the gate and lands on
/// the placeholder body — 501 Not Implemented (the disable function isn't
/// wired up in the MVP but the route is reserved + role-gated).
#[tokio::test]
async fn admin_token_allowed_on_admin_route_returns_501() {
    let kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test("test-kid", kp.public_key());
    let server = start_server_auth_enabled(jwks).await;

    let token = mint_role_token(
        &kp,
        "test-kid",
        None,
        Some(vec!["CmtraceOpen.Admin".to_string()]),
    );

    let resp = reqwest::Client::new()
        .post(format!(
            "{}/v1/admin/devices/some-device/disable",
            server.base
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let body = resp.text().await.expect("body");
    assert!(body.contains("not_implemented"), "got body: {body}");
}

/// Operator-only token on a query route still works — the existing happy
/// path stays intact under the new gate. (Belt-and-suspenders against
/// regressions in the role-extractor wiring.)
#[tokio::test]
async fn operator_token_allowed_on_query_route() {
    let kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test("test-kid", kp.public_key());
    let server = start_server_auth_enabled(jwks).await;

    let token = mint_role_token(
        &kp,
        "test-kid",
        Some("CmtraceOpen.Query"),
        None,
    );

    let resp = reqwest::Client::new()
        .get(format!("{}/v1/devices", server.base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
}

/// Admin-only token (no Query scope, no Operator app role) on a query
/// route — should still pass because admin implies operator. Locks in the
/// `OperatorPrincipal::has_role` upward implication.
#[tokio::test]
async fn admin_token_allowed_on_query_route_via_implication() {
    let kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test("test-kid", kp.public_key());
    let server = start_server_auth_enabled(jwks).await;

    let token = mint_role_token(
        &kp,
        "test-kid",
        None,
        Some(vec!["CmtraceOpen.Admin".to_string()]),
    );

    let resp = reqwest::Client::new()
        .get(format!("{}/v1/devices", server.base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
}

/// Dev-mode bypass (`CMTRACE_AUTH_MODE=disabled`) lets the synthetic
/// principal hit the admin route too — the principal carries both roles.
#[tokio::test]
async fn admin_route_allowed_without_bearer_when_disabled() {
    let server = start_server_auth_disabled().await;
    let resp = reqwest::Client::new()
        .post(format!(
            "{}/v1/admin/devices/some-device/disable",
            server.base
        ))
        .send()
        .await
        .expect("send");
    // 501 because the handler is a placeholder; what matters here is that
    // we got past the role gate (i.e. NOT 401/403).
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}
