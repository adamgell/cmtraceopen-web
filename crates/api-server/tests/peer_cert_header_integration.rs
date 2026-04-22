//! Integration tests for AppGW-terminated mTLS via `CMTRACE_PEER_CERT_HEADER`.
//!
//! These tests verify that when the api-server is configured with
//! `CMTRACE_PEER_CERT_HEADER` + `CMTRACE_TRUSTED_PROXY_CIDR`, the
//! `DeviceIdentity` extractor reads the client cert from the named HTTP
//! header rather than from an in-process TLS session.
//!
//! Requires the `test-mtls` feature (same gate as `mtls_integration`) so
//! rcgen is available for minting test certs and x509-parser is available
//! for the decode path. Run explicitly with:
//!
//! ```bash
//! cargo test -p api-server --features test-mtls --test peer_cert_header_integration
//! ```

#![cfg(feature = "test-mtls")]

use std::net::SocketAddr;
use std::sync::Arc;

use api_server::auth::device_identity::DeviceIdentitySource;
use api_server::router;
use api_server::state::{AppState, MtlsRuntimeConfig};
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use axum::extract::FromRequestParts;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::CertificateDer;
use tempfile::TempDir;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A CA-signed client cert for use in reverse-proxy header tests.
struct TestCert {
    _der: CertificateDer<'static>,
    pem: String,
    /// DER bytes of the CA that signed this cert. Pass this as
    /// `trusted_ca_ders` in `MtlsRuntimeConfig` so the extractor can
    /// re-validate the chain.
    ca_der: Vec<u8>,
}

fn mint_client_cert(tenant_id: &str, device_id: &str) -> TestCert {
    // Root CA.
    let mut ca_params = CertificateParams::new(vec![]).expect("ca params");
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "test-header-mtls-root");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .key_usages
        .extend([KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign]);
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-sign ca");

    // Client leaf with SAN URI device://{tenant}/{device_id}.
    let mut cli_params = CertificateParams::new(vec![]).expect("cli params");
    cli_params
        .distinguished_name
        .push(DnType::CommonName, device_id);
    cli_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ClientAuth);
    cli_params
        .subject_alt_names
        .push(SanType::URI(
            format!("device://{tenant_id}/{device_id}").try_into().unwrap(),
        ));
    let cli_key = KeyPair::generate().expect("cli key");
    let cli_cert = cli_params
        .signed_by(&cli_key, &ca_cert, &ca_key)
        .expect("sign cli");

    TestCert {
        _der: CertificateDer::from(cli_cert.der().to_vec()),
        pem: cli_cert.pem(),
        ca_der: ca_cert.der().to_vec(),
    }
}

struct TestServer {
    base: String,
    _tmp: TempDir,
}

async fn start_server_with_peer_cert_header(
    header_name: &str,
    trusted_cidr: &str,
    trusted_ca_ders: Vec<Vec<u8>>,
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

    let mtls = MtlsRuntimeConfig {
        require_on_ingest: false,
        expected_san_uri_scheme: "device".into(),
        peer_cert_header: Some(header_name.to_string()),
        trusted_proxy_cidr: Some(trusted_cidr.parse().expect("valid CIDR")),
        trusted_ca_ders,
    };

    let state = AppState::full(
        meta,
        blobs,
        "127.0.0.1:0".to_string(),
        api_server::auth::AuthState {
            mode: api_server::auth::AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(api_server::auth::JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        },
        Default::default(),
        mtls,
    );

    let app = router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    TestServer { base, _tmp: tmp }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Base64-encoded PEM in the header (the Azure AppGW default encoding where
/// the full PEM string is base64-encoded before insertion into the header)
/// → DeviceIdentity populated from the cert's SAN URI.
#[tokio::test]
async fn raw_pem_header_from_trusted_ip_yields_device_identity() {
    let tenant = "00000000-0000-0000-0000-000000000000";
    let device = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let cert = mint_client_cert(tenant, device);

    // 127.0.0.1 is the loopback address; use a /8 CIDR so the test server
    // running on localhost is always in the trusted range.
    let server = start_server_with_peer_cert_header(
        "X-ARR-ClientCert",
        "127.0.0.0/8",
        vec![cert.ca_der.clone()],
    ).await;
    let client = reqwest::Client::new();

    // AppGW sends the PEM base64-encoded so the header value is a single
    // line of printable ASCII with no embedded newlines.
    let b64_pem = STANDARD.encode(cert.pem.trim().as_bytes());

    // POST to the ingest bundle-init route with the cert in the header.
    // A missing/bad body returns 400 (not 401), proving DeviceIdentity
    // was extracted successfully from the header.
    let resp = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("X-ARR-ClientCert", &b64_pem)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");

    assert_ne!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "expected non-401 (cert should be accepted from trusted IP); got {}",
        resp.status()
    );
    // The handler validates the JSON body; bad body → 400/422, not 401.
    assert!(
        resp.status() == reqwest::StatusCode::BAD_REQUEST
            || resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400/422 from bundle-init validator; got {}",
        resp.status()
    );
}

/// Base64-encoded PEM in the header (AppGW default encoding) → accepted.
#[tokio::test]
async fn base64_pem_header_yields_device_identity() {
    let tenant = "11111111-0000-0000-0000-000000000000";
    let device = "22222222-bbbb-cccc-dddd-eeeeeeeeeeee";
    let cert = mint_client_cert(tenant, device);

    // Simulate AppGW base64-encoding the PEM string.
    let b64_pem = STANDARD.encode(cert.pem.trim().as_bytes());

    let server = start_server_with_peer_cert_header(
        "X-ARR-ClientCert",
        "127.0.0.0/8",
        vec![cert.ca_der.clone()],
    ).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("X-ARR-ClientCert", &b64_pem)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");

    assert_ne!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "base64-encoded PEM should be accepted; got {}",
        resp.status()
    );
}

/// No cert header → falls through to legacy X-Device-Id when not required.
#[tokio::test]
async fn missing_cert_header_falls_back_to_device_id_header() {
    // No CA DER needed here — no cert header is sent.
    let server = start_server_with_peer_cert_header(
        "X-ARR-ClientCert",
        "127.0.0.0/8",
        vec![],
    ).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("X-Device-Id", "WIN-FALLBACK-99")
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");

    // Expect 400/422, not 401: X-Device-Id fallback worked.
    assert_ne!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "X-Device-Id fallback should work when cert header absent; got {}",
        resp.status()
    );
}

/// A cert that was NOT signed by the configured CA bundle is rejected.
///
/// This test documents the defence-in-depth property introduced by comment
/// A in the review: the api-server re-validates the forwarded cert against
/// CMTRACE_CLIENT_CA_BUNDLE even though AppGW should have already verified
/// it. A misconfigured proxy (or a test proxy that forwards headers
/// unconditionally) cannot be used to inject an arbitrary self-signed cert
/// and claim a device identity.
#[tokio::test]
async fn cert_not_signed_by_trusted_ca_is_rejected() {
    let tenant = "55555555-0000-0000-0000-000000000000";
    let device = "66666666-bbbb-cccc-dddd-eeeeeeeeeeee";

    // Mint a cert signed by CA-A.
    let cert = mint_client_cert(tenant, device);

    // Server is configured to trust CA-B (a completely different CA).
    let other_cert = mint_client_cert("other-tenant", "other-device");
    let server = start_server_with_peer_cert_header(
        "X-ARR-ClientCert",
        "127.0.0.0/8",
        vec![other_cert.ca_der.clone()], // CA-B, not CA-A
    ).await;
    let client = reqwest::Client::new();

    // Send cert signed by CA-A to a server that trusts CA-B only.
    let b64_pem = STANDARD.encode(cert.pem.trim().as_bytes());
    let resp = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("X-ARR-ClientCert", &b64_pem)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("request");

    // The untrusted cert should be rejected — the extractor falls through
    // and returns 401 (no X-Device-Id header is sent).
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "cert signed by an untrusted CA must be rejected; got {}",
        resp.status()
    );
}

/// Unit-level test: extractor ignores cert header when ConnectInfo is absent
/// (no trusted-proxy gating possible → fail safe).
#[cfg(feature = "mtls")]
#[tokio::test]
async fn extractor_ignores_header_without_connect_info() {
    use api_server::auth::device_identity::DeviceIdentity;
    use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};

    let tenant = "33333333-0000-0000-0000-000000000000";
    let device = "44444444-bbbb-cccc-dddd-eeeeeeeeeeee";
    let cert = mint_client_cert(tenant, device);

    let tmp = TempDir::new().unwrap();
    let blobs = Arc::new(LocalFsBlobStore::new(tmp.path()).await.unwrap());
    let meta = Arc::new(SqliteMetadataStore::connect(":memory:").await.unwrap());

    let mtls = MtlsRuntimeConfig {
        require_on_ingest: false,
        expected_san_uri_scheme: "device".into(),
        peer_cert_header: Some("X-ARR-ClientCert".to_string()),
        trusted_proxy_cidr: Some("127.0.0.0/8".parse().unwrap()),
        trusted_ca_ders: vec![cert.ca_der.clone()],
    };
    let state = AppState::full(
        meta,
        blobs,
        "127.0.0.1:0".to_string(),
        api_server::auth::AuthState {
            mode: api_server::auth::AuthMode::Disabled,
            entra: None,
            jwks: Arc::new(api_server::auth::JwksCache::new(
                "http://127.0.0.1:1/unused".to_string(),
            )),
        },
        Default::default(),
        mtls,
    );

    // Build request with the cert header but NO ConnectInfo extension.
    // Use base64-encoded PEM so the header value is valid ASCII (no newlines).
    let b64_pem = STANDARD.encode(cert.pem.trim().as_bytes());
    let req = axum::http::Request::builder()
        .uri("/v1/ingest/bundles")
        .header("X-ARR-ClientCert", &b64_pem)
        .header("X-Device-Id", "WIN-FALLBACK-UNIT")
        .body(())
        .unwrap();
    let (mut parts, _) = req.into_parts();

    // The extractor should fall through to X-Device-Id (ConnectInfo absent
    // → cert header is treated as untrusted).
    let id = DeviceIdentity::from_request_parts(&mut parts, &state)
        .await
        .expect("should succeed via X-Device-Id fallback");

    assert_eq!(id.source, DeviceIdentitySource::HeaderTemp,
        "without ConnectInfo the cert header must be ignored");
    assert_eq!(id.device_id, "WIN-FALLBACK-UNIT");
}
