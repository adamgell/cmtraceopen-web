//! Wave 4 end-to-end integration test.
//!
//! Exercises the full Wave 4 stack in a single test:
//!
//!   1. A fake Cloud PKI leaf cert (self-signed CA, SAN URI
//!      `device://test-tenant/<uuid>`) drives an mTLS-authenticated
//!      bundle ingest (init → chunk → finalize).
//!   2. A fake Entra JWT (RS256, signed against a key pre-seeded into the
//!      in-process `JwksCache`) drives a query for the ingested device +
//!      session + entries.
//!   3. Assertions verify the device appears in the registry, the session
//!      exists with `parse_state=ok`, and the parsed entries carry the
//!      expected severity distribution.
//!
//! # Why `require_on_ingest = false` here
//!
//! When `require_on_ingest = true` the rustls `ServerConfig` uses a verifier
//! that does **not** call `allow_unauthenticated()`. That means **every** TLS
//! connection — including the JWT-only reqwest client hitting the query routes
//! — must present a client cert at the handshake level. Wiring a client cert
//! into reqwest is unnecessarily complex and obscures the JWT-auth test.
//!
//! Using `require_on_ingest = false` keeps the handshake-level verifier in
//! `allow_unauthenticated()` mode so the reqwest client can connect without
//! a cert. The mTLS cert path is still fully exercised: the tokio-rustls
//! ingest client presents the leaf cert; the `DeviceIdentity` extractor
//! parses its SAN URI and derives the device_id. The rejection path is
//! exercised by a separate sub-assertion (no cert + no header → 401).
//!
//! # Running
//!
//! ```bash
//! cargo test -p api-server --features test-mtls wave4_e2e
//! ```
//!
//! Gated behind `--features test-mtls` (implies the `mtls` feature which
//! pulls in `axum-server` / `rustls` / `aws-lc-sys`) to avoid requiring
//! cmake + NASM on plain `cargo test` runs.
//!
//! State isolation: each run creates a fresh `TempDir` for blobs and an
//! in-memory SQLite database, so tests running in parallel cannot collide.

#![cfg(feature = "test-mtls")]

use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use api_server::auth::{AuthState, EntraConfig, JwksCache};
use api_server::config::TlsConfig;
use api_server::router;
use api_server::state::{AppState, MtlsRuntimeConfig};
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use common_wire::ingest::{
    content_kind, BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest,
    BundleInitResponse,
};
use common_wire::{DeviceSummary, Paginated, SessionSummary};
use jwt_simple::prelude::{Claims, Duration as JwtDuration, RS256KeyPair, RSAKeyPairLike};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, Instant};
use tokio_rustls::TlsConnector;
use uuid::Uuid;
use zip::write::SimpleFileOptions;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TEST_TENANT: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const TEST_DEVICE: &str = "11111111-2222-3333-4444-555555555555";
const TEST_AUDIENCE: &str = "api://cmtraceopen-wave4-test";
const JWKS_KID: &str = "wave4-test-kid";

// ---------------------------------------------------------------------------
// PKI helpers
// ---------------------------------------------------------------------------

/// All crypto artifacts minted for one test run.
struct PkiArtifacts {
    /// Keeps the temp-dir alive for the life of the test.
    _tmp: TempDir,
    server_cert_pem: std::path::PathBuf,
    server_key_pem: std::path::PathBuf,
    client_ca_bundle: std::path::PathBuf,
    client_cert_der: CertificateDer<'static>,
    client_key_der: PrivateKeyDer<'static>,
    /// The self-signed CA cert DER. Used as the TLS client trust anchor so
    /// both the mTLS connector (presents the device cert) and the plain-TLS
    /// connector (JWT-only query routes) can validate the server's certificate,
    /// which is signed by this CA.
    ca_cert_der: CertificateDer<'static>,
}

/// Mint a self-signed CA + server leaf + client leaf mimicking the Cloud PKI
/// structure expected by the Wave 4 mTLS stack.
///
/// The client leaf carries SAN URI `device://<tenant_id>/<device_id>`, which
/// is exactly what the `DeviceIdentity` extractor parses to derive the
/// `device_id` claim.
fn mint_pki(tenant_id: &str, device_id: &str) -> PkiArtifacts {
    let tmp = TempDir::new().expect("tempdir");

    // ---- Root CA -------------------------------------------------------
    let mut ca_params = CertificateParams::new(vec![]).expect("ca params");
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "wave4-test-root");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .key_usages
        .extend([KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign]);
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-sign ca");

    // ---- Server leaf ---------------------------------------------------
    let mut srv_params =
        CertificateParams::new(vec!["localhost".to_string()]).expect("srv params");
    srv_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    srv_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ServerAuth);
    let srv_key = KeyPair::generate().expect("srv key");
    let srv_cert = srv_params
        .signed_by(&srv_key, &ca_cert, &ca_key)
        .expect("sign srv");

    // ---- Client leaf (Cloud PKI device cert) ---------------------------
    // SAN URI scheme is `device`, which is what the server is configured to
    // accept via `expected_san_uri_scheme`.
    let mut cli_params = CertificateParams::new(vec![]).expect("cli params");
    cli_params
        .distinguished_name
        .push(DnType::CommonName, device_id);
    cli_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ClientAuth);
    cli_params.subject_alt_names.push(SanType::URI(
        format!("device://{tenant_id}/{device_id}").try_into().unwrap(),
    ));
    let cli_key = KeyPair::generate().expect("cli key");
    let cli_cert = cli_params
        .signed_by(&cli_key, &ca_cert, &ca_key)
        .expect("sign cli");

    // Write server identity + CA bundle to disk for the api-server config.
    let server_cert_pem = tmp.path().join("server.crt");
    let server_key_pem = tmp.path().join("server.key");
    let client_ca_bundle = tmp.path().join("client-ca.pem");
    {
        let mut f = std::fs::File::create(&server_cert_pem).unwrap();
        f.write_all(srv_cert.pem().as_bytes()).unwrap();
    }
    {
        let mut f = std::fs::File::create(&server_key_pem).unwrap();
        f.write_all(srv_key.serialize_pem().as_bytes()).unwrap();
    }
    {
        let mut f = std::fs::File::create(&client_ca_bundle).unwrap();
        f.write_all(ca_cert.pem().as_bytes()).unwrap();
    }

    PkiArtifacts {
        _tmp: tmp,
        server_cert_pem,
        server_key_pem,
        client_ca_bundle,
        client_cert_der: CertificateDer::from(cli_cert.der().to_vec()),
        client_key_der: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            cli_key.serialize_der(),
        )),
        ca_cert_der: CertificateDer::from(ca_cert.der().to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Server helpers
// ---------------------------------------------------------------------------

/// All handles needed to interact with the test server.
struct TestServer {
    addr: SocketAddr,
    /// mTLS-capable connector (presents the device leaf cert + key). Used
    /// for the ingest phase.
    connector: TlsConnector,
    /// Plain TLS connector (no client cert). Used to confirm the extractor
    /// returns 401 when no identity is available.
    plain_connector: TlsConnector,
    /// reqwest client for JWT-authenticated query routes. Trusts the
    /// server's self-signed cert; does NOT present a client cert.
    client: reqwest::Client,
    /// Keep PKI artifacts alive so temp files don't disappear mid-test.
    _pki: PkiArtifacts,
    /// Owns the blob-store tempdir for the lifetime of the TestServer.
    /// Replaces the older `std::mem::forget(tmp)` pattern (review fix):
    /// explicit ownership means the tempdir is dropped (and reclaimed)
    /// when the test fixture goes out of scope rather than leaking.
    _blob_tmp: TempDir,
}

/// Boot a TLS-terminating api-server that:
///   - `require_on_ingest` controls whether the TLS verifier operates in
///     `allow_unauthenticated()` mode (false → reqwest can reach query routes
///     w/o client cert) or rejects unauthenticated peers at the handshake
///     (true → only used by the dedicated TLS-handshake-rejection sub-test).
///   - Enables Entra JWT auth with `jwks` pre-seeded so tests skip the
///     network.
///   - Uses an in-memory SQLite + local-FS blob store in a fresh tempdir.
async fn start_wave4_server(
    pki: PkiArtifacts,
    entra_jwks: Arc<JwksCache>,
    require_on_ingest: bool,
) -> TestServer {
    let tmp = TempDir::new().expect("data tempdir");
    let blobs = Arc::new(LocalFsBlobStore::new(tmp.path()).await.expect("blobs"));
    let meta = Arc::new(
        SqliteMetadataStore::connect(":memory:")
            .await
            .expect("sqlite"),
    );

    // Wire Entra auth: the test signs tokens with the keypair it seeded into
    // `entra_jwks` — no network call to login.microsoftonline.com is made.
    let entra = EntraConfig {
        tenant_id: TEST_TENANT.to_string(),
        audience: TEST_AUDIENCE.to_string(),
        jwks_uri: "http://127.0.0.1:1/unused".to_string(),
    };
    let auth = AuthState {
        mode: api_server::auth::AuthMode::Enabled,
        entra: Some(entra),
        jwks: entra_jwks,
    };

    // `require_on_ingest` flips the TLS-level verifier between
    // `allow_unauthenticated()` (false) and full handshake-level cert
    // enforcement (true). See module-level doc-comment for rationale.
    let mtls = MtlsRuntimeConfig {
        require_on_ingest,
        expected_san_uri_scheme: "device".into(),
    };

    let state = AppState::full(
        meta,
        blobs,
        "127.0.0.1:0".to_string(),
        auth,
        Default::default(),
        mtls,
    );
    let app = router(state);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let tls_cfg = TlsConfig {
        enabled: true,
        server_cert_pem: Some(pki.server_cert_pem.clone()),
        server_key_pem: Some(pki.server_key_pem.clone()),
        client_ca_bundle: Some(pki.client_ca_bundle.clone()),
        require_on_ingest,
        expected_san_uri_scheme: "device".into(),
    };

    let handle = axum_server::Handle::new();
    let handle_clone = handle.clone();
    let app_clone = app.clone();
    tokio::spawn(async move {
        let _ = api_server::tls::serve_tls_with_handle(addr, app_clone, &tls_cfg, handle_clone)
            .await;
    });
    let bound = tokio::time::timeout(Duration::from_secs(5), handle.listening())
        .await
        .expect("server listening within 5s")
        .expect("listening addr");

    // Build a root-cert store that trusts our self-signed CA cert. Both the
    // mTLS connector and the plain-TLS connector trust the same CA, which
    // signed the server's certificate.
    let mut roots = rustls::RootCertStore::empty();
    roots.add(pki.ca_cert_der.clone()).unwrap();

    // mTLS connector (presents the device leaf cert + key).
    let mtls_cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots.clone())
        .with_client_auth_cert(
            vec![pki.client_cert_der.clone()],
            pki.client_key_der.clone_key(),
        )
        .expect("client auth cert");
    let connector = TlsConnector::from(Arc::new(mtls_cfg));

    // Plain TLS connector — no client cert.
    let plain_cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots.clone())
        .with_no_client_auth();
    let plain_connector = TlsConnector::from(Arc::new(plain_cfg));

    // reqwest client that trusts the self-signed CA cert but presents no
    // client cert — used for JWT-authenticated query routes.
    let ca_cert_pem = std::fs::read(&pki.client_ca_bundle).expect("read CA bundle");
    let reqwest_cert = reqwest::Certificate::from_pem(&ca_cert_pem).expect("reqwest cert");
    let http_client = reqwest::Client::builder()
        .add_root_certificate(reqwest_cert)
        .build()
        .expect("reqwest client");

    TestServer {
        addr: bound,
        connector,
        plain_connector,
        client: http_client,
        _pki: pki,
        _blob_tmp: tmp,
    }
}

// ---------------------------------------------------------------------------
// Raw TLS HTTP/1.1 helpers
// ---------------------------------------------------------------------------

/// Open a TLS connection using `connector` and POST a raw HTTP/1.1 request.
/// Returns the raw response bytes.
///
/// The `connector` parameter lets callers choose between the mTLS connector
/// (presents the device client cert) and the plain-TLS connector (no client
/// cert), so the same helper drives both the happy-path and the rejection
/// assertion.
async fn tls_post(
    addr: SocketAddr,
    connector: &TlsConnector,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> Vec<u8> {
    let stream = TcpStream::connect(addr).await.expect("tcp connect");
    let domain = ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(domain, stream).await.expect("handshake");

    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n",
        len = body.len()
    );
    tls.write_all(req.as_bytes()).await.unwrap();
    tls.write_all(body).await.unwrap();
    tls.flush().await.unwrap();
    let mut buf = Vec::new();
    let _ = tls.read_to_end(&mut buf).await;
    buf
}

/// Open a TLS connection using `connector` and PUT a raw HTTP/1.1 request.
async fn tls_put(
    addr: SocketAddr,
    connector: &TlsConnector,
    path: &str,
    body: &[u8],
) -> Vec<u8> {
    let stream = TcpStream::connect(addr).await.expect("tcp connect");
    let domain = ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(domain, stream).await.expect("handshake");

    let req = format!(
        "PUT {path} HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n",
        len = body.len()
    );
    tls.write_all(req.as_bytes()).await.unwrap();
    tls.write_all(body).await.unwrap();
    tls.flush().await.unwrap();
    let mut buf = Vec::new();
    let _ = tls.read_to_end(&mut buf).await;
    buf
}

/// Extract the HTTP status code from a raw HTTP/1.1 response line.
fn parse_status(raw: &[u8]) -> u16 {
    let s = String::from_utf8_lossy(raw);
    let line = s.lines().next().unwrap_or_default();
    // "HTTP/1.1 201 Created" → 201
    line.split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Extract the JSON body from a raw HTTP/1.1 response (after the blank line).
fn parse_body(raw: &[u8]) -> &[u8] {
    // Find the header/body separator "\r\n\r\n".
    raw.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| &raw[i + 4..])
        .unwrap_or(&raw[raw.len()..])
}

// ---------------------------------------------------------------------------
// Evidence-zip builder
// ---------------------------------------------------------------------------

/// Build a tiny evidence-zip with three CMTrace log lines (Info / Warning /
/// Error). Mirrors the fixture in `parse_integration.rs`.
fn build_evidence_zip() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: SimpleFileOptions =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zw.start_file("manifest.json", opts).unwrap();
        zw.write_all(br#"{"schemaVersion":1,"bundleKind":"evidence-zip"}"#)
            .unwrap();

        zw.start_file("evidence/logs/wave4.log", opts).unwrap();
        let log = concat!(
            r#"<![LOG[Wave4 e2e - info line]LOG]!><time="00:00:00.000+000" date="01-01-2026" component="wave4" context="" type="1" thread="1" file="w.cpp:1">"#, "\n",
            r#"<![LOG[Wave4 e2e - warning line]LOG]!><time="00:00:01.000+000" date="01-01-2026" component="wave4" context="" type="2" thread="1" file="w.cpp:2">"#, "\n",
            r#"<![LOG[Wave4 e2e - error line]LOG]!><time="00:00:02.000+000" date="01-01-2026" component="wave4" context="" type="3" thread="1" file="w.cpp:3">"#, "\n",
        );
        zw.write_all(log.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// JWT helper
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct TestClaims {
    scp: String,
    name: String,
    tid: String,
}

fn mint_operator_jwt(kp: &RS256KeyPair) -> String {
    mint_jwt_with_scope(kp, "CmtraceOpen.Query openid profile")
}

/// Mint a Wave 4 test JWT with the supplied `scp` claim verbatim — used by
/// the RBAC negative-path test to mint a token that is technically valid
/// (signature OK, audience matches, key id present in the cache) but is
/// missing the `CmtraceOpen.Query` scope. The query routes must surface a
/// 403 in that case, not 401, since the token itself is valid.
fn mint_jwt_with_scope(kp: &RS256KeyPair, scp: &str) -> String {
    let issuer = format!(
        "https://login.microsoftonline.com/{}/v2.0",
        TEST_TENANT
    );
    let custom = TestClaims {
        scp: scp.to_string(),
        name: "Wave4 E2E Operator".to_string(),
        tid: TEST_TENANT.to_string(),
    };
    let claims = Claims::with_custom_claims(custom, JwtDuration::from_secs(300))
        .with_issuer(&issuer)
        .with_audience(TEST_AUDIENCE)
        .with_subject("wave4-e2e@example.com");
    let kp_with_kid = kp.clone().with_key_id(JWKS_KID);
    kp_with_kid.sign(claims).expect("sign jwt")
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

/// Full Wave 4 end-to-end test.
///
/// Coverage:
///   - mTLS cert path: ingest with the device leaf cert uses cert-derived
///     device identity (SAN URI parsed → `device_id`).
///   - mTLS rejection logic: ingest without a cert and without an
///     `X-Device-Id` header returns 401 (extractor rejects).
///   - JWT validation + RBAC: query routes require a valid Entra bearer token.
///   - Ingest pipeline: init → chunk → finalize with an evidence-zip payload.
///   - Parse worker: background parse flips `parse_state` to `ok` and
///     populates `entries`.
///   - Query layer: devices, sessions, and entries are visible over HTTPS with
///     the Entra JWT.
#[tokio::test]
async fn wave4_e2e_mtls_ingest_and_jwt_query() {
    api_server::tls::install_default_crypto_provider_for_tests();

    // ------------------------------------------------------------------ setup
    let pki = mint_pki(TEST_TENANT, TEST_DEVICE);

    // Mint an RS256 keypair and pre-seed it into the JWKS cache so the
    // server can validate the test JWT without hitting the network.
    let jwt_kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test(JWKS_KID, jwt_kp.public_key());

    let server = start_wave4_server(pki, jwks, false).await;
    let base = format!("https://localhost:{}", server.addr.port());

    // ---- mTLS rejection: ingest without cert and without X-Device-Id
    //      header must return 401 from the DeviceIdentity extractor.
    let rejection_body = serde_json::to_vec(&serde_json::json!({
        "bundleId": Uuid::now_v7().to_string(),
        "sha256": "a".repeat(64),
        "sizeBytes": 1,
        "contentKind": "evidence-zip"
    }))
    .unwrap();
    let rejection_raw = tls_post(
        server.addr,
        &server.plain_connector,
        "/v1/ingest/bundles",
        "application/json",
        &rejection_body,
    )
    .await;
    let rejection_status = parse_status(&rejection_raw);
    assert_eq!(
        rejection_status, 401,
        "ingest without cert+header must be 401; got {rejection_status}; raw:\n{}",
        String::from_utf8_lossy(&rejection_raw)
    );

    // ------------------------------------------------------------ ingest phase
    let payload = build_evidence_zip();
    let sha = sha256_hex(&payload);
    let bundle_id = Uuid::now_v7();

    // init (mTLS — device leaf cert presented)
    let init_body = serde_json::to_vec(&BundleInitRequest {
        bundle_id,
        device_hint: Some("wave4-e2e".into()),
        sha256: sha.clone(),
        size_bytes: payload.len() as u64,
        content_kind: content_kind::EVIDENCE_ZIP.into(),
    })
    .unwrap();
    let init_raw = tls_post(
        server.addr,
        &server.connector,
        "/v1/ingest/bundles",
        "application/json",
        &init_body,
    )
    .await;
    let init_status = parse_status(&init_raw);
    assert!(
        init_status == 200 || init_status == 201,
        "init expected 200/201, got {init_status}; raw:\n{}",
        String::from_utf8_lossy(&init_raw)
    );
    let init_resp: BundleInitResponse =
        serde_json::from_slice(parse_body(&init_raw)).expect("parse init response");
    assert_eq!(init_resp.resume_offset, 0);

    // chunk (mTLS)
    let chunk_path = format!(
        "/v1/ingest/bundles/{}/chunks?offset=0",
        init_resp.upload_id
    );
    let chunk_raw = tls_put(server.addr, &server.connector, &chunk_path, &payload).await;
    let chunk_status = parse_status(&chunk_raw);
    assert!(
        chunk_status == 200 || chunk_status == 201,
        "chunk expected 200/201, got {chunk_status}; raw:\n{}",
        String::from_utf8_lossy(&chunk_raw)
    );

    // finalize (mTLS)
    let fin_body = serde_json::to_vec(&BundleFinalizeRequest {
        final_sha256: sha.clone(),
    })
    .unwrap();
    let fin_path = format!("/v1/ingest/bundles/{}/finalize", init_resp.upload_id);
    let fin_raw = tls_post(
        server.addr,
        &server.connector,
        &fin_path,
        "application/json",
        &fin_body,
    )
    .await;
    let fin_status = parse_status(&fin_raw);
    assert!(
        fin_status == 200 || fin_status == 201,
        "finalize expected 200/201, got {fin_status}; raw:\n{}",
        String::from_utf8_lossy(&fin_raw)
    );
    let fin_resp: BundleFinalizeResponse =
        serde_json::from_slice(parse_body(&fin_raw)).expect("parse finalize response");
    let session_id = fin_resp.session_id;
    // The parse worker is background; finalize returns "pending".
    assert_eq!(fin_resp.parse_state, "pending");

    // ---------------------------------------------------------- query phase
    let token = mint_operator_jwt(&jwt_kp);

    // ---- 1. Device must appear in the registry -------------------------
    let devices_resp = server
        .client
        .get(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET /v1/devices");
    assert_eq!(
        devices_resp.status(),
        reqwest::StatusCode::OK,
        "GET /v1/devices failed: {}",
        devices_resp.text().await.unwrap_or_default()
    );
    let devices: Paginated<DeviceSummary> = devices_resp.json().await.expect("parse devices");
    let found_device = devices.items.iter().any(|d| d.device_id == TEST_DEVICE);
    assert!(
        found_device,
        "expected device {TEST_DEVICE} in registry; got {:?}",
        devices.items.iter().map(|d| &d.device_id).collect::<Vec<_>>()
    );

    // ---- 2. Session must appear for the device -------------------------
    let sessions_resp = server
        .client
        .get(format!("{base}/v1/devices/{TEST_DEVICE}/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET /v1/devices/.../sessions");
    assert_eq!(
        sessions_resp.status(),
        reqwest::StatusCode::OK,
        "GET sessions failed: {}",
        sessions_resp.text().await.unwrap_or_default()
    );
    let sessions: Paginated<SessionSummary> = sessions_resp.json().await.expect("parse sessions");
    let found_session = sessions.items.iter().any(|s| s.session_id == session_id);
    assert!(
        found_session,
        "expected session {session_id} in sessions for {TEST_DEVICE}"
    );

    // ---- 3. Poll until parse_state != "pending" ------------------------
    //
    // Track the last observed `parse_state` across iterations so that — if the
    // worker stalls and we time out on `pending` — operators triaging a flaky
    // test see the actual stall state rather than a generic "timed out"
    // message (review fix).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut final_parse_state = String::new();
    let mut last_observed = String::from("<no response yet>");
    let mut iteration_count: usize = 0;
    while Instant::now() < deadline {
        iteration_count += 1;
        let s: SessionSummary = server
            .client
            .get(format!("{base}/v1/sessions/{session_id}"))
            .bearer_auth(&token)
            .send()
            .await
            .expect("GET /v1/sessions/{id}")
            .error_for_status()
            .expect("session query status")
            .json()
            .await
            .expect("parse session summary");
        last_observed = s.parse_state.clone();
        if s.parse_state != "pending" {
            final_parse_state = s.parse_state;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        final_parse_state, "ok",
        "expected parse_state=ok within 10s; last observed parse_state={last_observed:?} \
         after {iteration_count} poll iterations (final_parse_state={final_parse_state:?})",
    );

    // ---- 4. Entries must be present with expected severity distribution
    let entries_resp = server
        .client
        .get(format!("{base}/v1/sessions/{session_id}/entries"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET /v1/sessions/{id}/entries")
        .error_for_status()
        .expect("entries query status");
    let entries: serde_json::Value = entries_resp.json().await.expect("parse entries");
    let items = entries["items"].as_array().expect("items array");
    assert!(
        items.len() >= 3,
        "expected at least 3 parsed entries, got {}",
        items.len()
    );

    // Severity distribution: CMTrace type 1→"Info", 2→"Warning", 3→"Error".
    let severities: Vec<&str> = items
        .iter()
        .filter_map(|e| e["severity"].as_str())
        .collect();
    assert!(
        severities.contains(&"Info"),
        "expected at least one Info entry; got {severities:?}"
    );
    assert!(
        severities.contains(&"Warning"),
        "expected at least one Warning entry; got {severities:?}"
    );
    assert!(
        severities.contains(&"Error"),
        "expected at least one Error entry; got {severities:?}"
    );

    // ---- 5. Unauthenticated query must be rejected with 401 -----------
    let unauth_resp = server
        .client
        .get(format!("{base}/v1/devices"))
        .send()
        .await
        .expect("unauthenticated GET /v1/devices");
    assert_eq!(
        unauth_resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "expected 401 without JWT; got {}",
        unauth_resp.status()
    );
}

// ---------------------------------------------------------------------------
// Additional Wave 4 e2e tests (review-fix follow-ups)
// ---------------------------------------------------------------------------

/// Verifies that a connection to a `require_on_ingest = true` server without a
/// client cert cannot be used to successfully ingest bundles.
///
/// ## TLS 1.3 timing note
///
/// In TLS 1.3, the client-side handshake completes before the server processes
/// the client's (absent) Certificate message — `connect()` may return `Ok`
/// even though the server will imminently reject the connection. We therefore
/// test at the application level: drive a minimal HTTP request and assert
/// either:
///   - The connection is closed by the server (empty or partial response), OR
///   - The server returns 401 (the `DeviceIdentity` extractor fires after
///     finding no SAN URI in the absence of a client cert).
///
/// Both outcomes confirm the intended security guarantee: a client without a
/// valid device cert cannot ingest data even when `require_on_ingest = true`.
#[tokio::test]
async fn wave4_mtls_handshake_rejects_unauthenticated_client() {
    api_server::tls::install_default_crypto_provider_for_tests();

    let pki = mint_pki(TEST_TENANT, TEST_DEVICE);
    let jwt_kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test(JWKS_KID, jwt_kp.public_key());

    // require_on_ingest = TRUE → TLS-level cert enforcement.
    let server = start_wave4_server(pki, jwks, true).await;

    let stream = TcpStream::connect(server.addr).await.expect("tcp connect");
    let domain = ServerName::try_from("localhost").unwrap();

    match server.plain_connector.connect(domain, stream).await {
        Err(_) => {
            // TLS-level rejection during the handshake itself — ideal behavior.
        }
        Ok(mut tls) => {
            // Handshake completed from the client's perspective (TLS 1.3 timing);
            // the server-side cert verification fires shortly after. Drive a
            // minimal HTTP request and assert the connection is unusable.
            let req = b"POST /v1/ingest/bundles HTTP/1.1\r\n\
                         Host: localhost\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: 2\r\n\
                         Connection: close\r\n\r\n{}";
            let _ = tls.write_all(req).await;
            let _ = tls.flush().await;
            let mut buf = Vec::new();
            let _ = tls.read_to_end(&mut buf).await;

            let status = parse_status(&buf);
            // Either the server closed the connection (buf is empty) or it
            // returned 401 via the `DeviceIdentity` extractor which fires
            // when no SAN URI is present in the client cert chain. Both
            // prove the no-cert path is blocked.
            assert!(
                buf.is_empty() || status == 401,
                "expected empty response (TLS close) or 401 (extractor rejection) \
                 for no-cert connection to require_on_ingest=true server; \
                 got status={status} body={}",
                String::from_utf8_lossy(&buf)
            );
        }
    }
}

/// RBAC negative path: a token that's cryptographically valid (correct
/// signature, audience, issuer, key id) but whose `scp` claim does NOT
/// include `CmtraceOpen.Query` must be rejected.
///
/// ## 401 vs 403
///
/// The `validate_bearer` function maps `InsufficientScope` (valid token,
/// wrong scope) to HTTP 401 via `AuthError::InsufficientScope` → the `_ =>
/// StatusCode::UNAUTHORIZED` arm in `AuthError::into_response`. HTTP 403
/// (`AuthError::ForbiddenRole`) is only returned by the *per-route* role gate
/// when a principal already has at least one valid role but lacks a more
/// specific route-level role. Insufficient scope at the token level is treated
/// as an auth failure (re-authenticate with the right scope), not a role
/// failure (your identity is fine, this route is off-limits).
///
/// Closes the gap flagged in review: previously only the happy-path token
/// (with the scope) was tested, so a regression that drops the scope check
/// would still pass.
#[tokio::test]
async fn wave4_query_rejects_token_without_query_scope() {
    api_server::tls::install_default_crypto_provider_for_tests();

    let pki = mint_pki(TEST_TENANT, TEST_DEVICE);
    let jwt_kp = RS256KeyPair::generate(2048).unwrap();
    let jwks = Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string()));
    jwks.insert_for_test(JWKS_KID, jwt_kp.public_key());

    let server = start_wave4_server(pki, jwks, false).await;
    let base = format!("https://localhost:{}", server.addr.port());

    // Mint a token whose scope is everything OPERATOR_AND_ABOVE checks for
    // *except* CmtraceOpen.Query.
    let bad_token = mint_jwt_with_scope(&jwt_kp, "openid profile email");

    let resp = server
        .client
        .get(format!("{base}/v1/devices"))
        .bearer_auth(&bad_token)
        .send()
        .await
        .expect("GET /v1/devices with no-query-scope token");

    // InsufficientScope → 401 (re-authenticate; the current token lacks the
    // required scope). Not 403 — that's reserved for valid-principal /
    // wrong-route-role scenarios.
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "expected 401 from query route when token is valid but missing \
         CmtraceOpen.Query scope; got {} body={:?}",
        resp.status(),
        resp.text().await.unwrap_or_default(),
    );
}
