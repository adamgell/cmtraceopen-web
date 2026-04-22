//! End-to-end integration: collect a small bundle on the current
//! platform, enqueue it, run the uploader against an in-process
//! api-server, and confirm it shows up as a session on the server.
//!
//! Intentionally Linux-friendly — the only collector we exercise is
//! `LogsCollector` (works on any OS). The Windows-only collectors
//! return `NotSupported` manifests that are still zipped into the
//! bundle; that matches how the Linux CI runner will see a real
//! bundle payload.

use std::sync::Arc;
use std::time::Duration;

use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use cmtraceopen_agent::collectors::dsregcmd::DsRegCmdCollector;
use cmtraceopen_agent::collectors::event_logs::EventLogsCollector;
use cmtraceopen_agent::collectors::evidence::EvidenceOrchestrator;
use cmtraceopen_agent::collectors::logs::LogsCollector;
use cmtraceopen_agent::queue::{Queue, QueueState};
use cmtraceopen_agent::tls::TlsClientOptions;
use cmtraceopen_agent::uploader::{RetryPolicy, Uploader, UploaderConfig};
use common_wire::{Paginated, SessionSummary};
use tempfile::TempDir;
use tokio::net::TcpListener;

struct TestServer {
    base: String,
    _tmp: TempDir,
}

async fn start_server() -> TestServer {
    // The api-server crate uses reqwest with rustls-tls-native-roots-no-provider
    // (PR #46). Constructing its router eagerly builds a reqwest client for the
    // JWKS cache, which panics with "No provider set" unless a rustls crypto
    // provider is installed first. Uploader::new() also installs it, but it
    // runs AFTER start_server() in the test body — so we install here too
    // (idempotent via OnceLock).
    cmtraceopen_agent::tls::install_default_crypto_provider();
    let tmp = TempDir::new().expect("tempdir");
    let blobs = Arc::new(LocalFsBlobStore::new(tmp.path()).await.expect("blob store"));
    let meta = Arc::new(
        SqliteMetadataStore::connect(":memory:")
            .await
            .expect("sqlite"),
    );
    // Use the auth-disabled helper: this end-to-end test exercises the agent
    // upload path + a follow-up query, but doesn't validate the operator-bearer
    // surface (covered separately in `auth_integration.rs`).
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

/// Collect → enqueue → upload → assert session visible.
#[tokio::test]
async fn agent_ships_bundle_to_api_server_end_to_end() {
    let server = start_server().await;

    // --- seed a fake log directory the agent will walk ---
    let source = TempDir::new().unwrap();
    std::fs::write(
        source.path().join("ccmexec.log"),
        b"<![LOG[agent e2e smoke]LOG]!>\r\n",
    )
    .unwrap();
    let pattern = format!(
        "{}/*.log",
        source.path().to_string_lossy().replace('\\', "/")
    );

    // --- build the orchestrator with the single Linux-friendly collector ---
    let work = TempDir::new().unwrap();
    let orch = EvidenceOrchestrator::new(
        LogsCollector::new(vec![pattern]),
        EventLogsCollector::with_defaults(), // NotSupported on Linux
        DsRegCmdCollector::new(),            // NotSupported on Linux
        work.path().to_path_buf(),
    );

    let bundle = orch.collect_once().await.expect("collect");
    assert!(bundle.metadata.size_bytes > 0);
    let bundle_id = bundle.metadata.bundle_id;

    // --- enqueue ---
    let queue_dir = TempDir::new().unwrap();
    let queue = Queue::open(queue_dir.path()).await.expect("queue open");
    queue
        .enqueue(bundle.metadata.clone(), &bundle.zip_path)
        .await
        .expect("enqueue");

    let pending = queue
        .next_pending()
        .await
        .expect("next_pending")
        .expect("at least one pending");
    assert_eq!(pending.metadata.bundle_id, bundle_id);

    // --- upload ---
    // `RetryPolicy::immediate` keeps the test fast if there's a blip; the
    // local loopback server shouldn't produce any. The default
    // `TlsClientOptions` (native roots, no client cert) is fine for the
    // `http://` loopback URL — rustls only kicks in for `https://`, so
    // the integration suite is unaffected by the TLS rework.
    //
    // TODO(wave-3): once the api-server gains an mTLS-required mode,
    // add a parallel integration test that boots it with mutual TLS,
    // generates a throwaway cert/key with rcgen, and asserts the agent
    // round-trips a bundle while presenting that cert.
    let cfg = UploaderConfig {
        endpoint: server.base.clone(),
        device_id: "WIN-E2E-01".into(),
        request_timeout: Duration::from_secs(10),
        retry: RetryPolicy::immediate(3),
        tls: TlsClientOptions::default(),
    };
    let uploader = Uploader::new(cfg).expect("uploader");
    queue.mark_uploading(bundle_id).await.unwrap();
    let resp = uploader
        .upload(&pending.metadata, &pending.zip_path)
        .await
        .expect("upload succeeded");
    queue.mark_done(bundle_id).await.unwrap();

    // --- assert server side ---
    let client = reqwest::Client::new();
    let list: Paginated<SessionSummary> = client
        .get(format!(
            "{}/v1/devices/WIN-E2E-01/sessions?limit=10",
            server.base
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(list.items.len(), 1, "exactly one session");
    let got = &list.items[0];
    assert_eq!(got.session_id, resp.session_id);
    assert_eq!(got.bundle_id, bundle_id);
    assert_eq!(got.size_bytes, pending.metadata.size_bytes);

    // --- and the queue reflects the Done state ---
    let final_state = queue.get(bundle_id).await.unwrap();
    assert!(matches!(final_state.state, QueueState::Done { .. }));
}
