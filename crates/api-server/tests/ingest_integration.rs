//! End-to-end integration tests for the bundle-ingest flow.
//!
//! Each test spins up the real Axum server on an ephemeral loopback port
//! backed by a tempdir + in-memory SQLite, then drives init → chunk(s) →
//! finalize via reqwest and verifies the session shows up in the device's
//! session list.

use std::sync::Arc;

use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use common_wire::ingest::{
    content_kind, BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest,
    BundleInitResponse,
};
use common_wire::{Paginated, SessionSummary};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use uuid::Uuid;

/// Boots the server, returns the base URL (e.g. http://127.0.0.1:XYZ) along
/// with the tempdir whose Drop must outlive the test.
struct TestServer {
    base: String,
    // Kept alive for the life of the test; cleaned up on drop.
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
    let state = AppState::new(meta, blobs);
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

#[tokio::test]
async fn happy_path_single_chunk() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-HAPPY-01";
    let payload = b"hello bundle";
    let sha = sha256_hex(payload);
    let bundle_id = Uuid::now_v7();

    // init
    let init: BundleInitResponse = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: Some("lab01".into()),
            sha256: sha.clone(),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(init.resume_offset, 0);
    assert!(init.chunk_size > 0);

    // chunk
    let resp = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .header("content-type", "application/octet-stream")
        .body(payload.to_vec())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "chunk status: {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["nextOffset"], payload.len() as u64);

    // finalize
    let fin: BundleFinalizeResponse = client
        .post(format!(
            "{}/v1/ingest/bundles/{}/finalize",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .json(&BundleFinalizeRequest {
            final_sha256: sha.clone(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(fin.parse_state, "pending");

    // Session visible in the device's session list.
    let list: Paginated<SessionSummary> = client
        .get(format!(
            "{}/v1/devices/{}/sessions?limit=10",
            server.base, device_id
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].session_id, fin.session_id);
    assert_eq!(list.items[0].bundle_id, bundle_id);
    assert_eq!(list.items[0].size_bytes, payload.len() as u64);
    assert_eq!(list.items[0].parse_state, "pending");

    // And reachable directly.
    let one: SessionSummary = client
        .get(format!("{}/v1/sessions/{}", server.base, fin.session_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one.session_id, fin.session_id);
}

#[tokio::test]
async fn happy_path_multi_chunk_and_bad_offset_rejected() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-MULTI-02";
    // 20 KiB of deterministic bytes so we can chunk non-trivially.
    let payload: Vec<u8> = (0..20_480u32).map(|i| (i % 251) as u8).collect();
    let sha = sha256_hex(&payload);
    let bundle_id = Uuid::now_v7();

    let init: BundleInitResponse = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha.clone(),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    // Chunk 1: first 8 KiB.
    let part1 = &payload[..8192];
    let r1 = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(part1.to_vec())
        .send()
        .await
        .unwrap();
    assert!(r1.status().is_success());

    // Bad offset: server cursor is 8192 now, so offset=0 should 409.
    let bad = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(part1.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::CONFLICT);

    // Chunk 2: remaining bytes at correct offset.
    let part2 = &payload[8192..];
    let r2 = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=8192",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(part2.to_vec())
        .send()
        .await
        .unwrap();
    assert!(r2.status().is_success());

    // Finalize succeeds with matching sha.
    let fin_resp = client
        .post(format!(
            "{}/v1/ingest/bundles/{}/finalize",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .json(&BundleFinalizeRequest {
            final_sha256: sha.clone(),
        })
        .send()
        .await
        .unwrap();
    assert!(fin_resp.status().is_success());

    // Missing X-Device-Id on a protected route is a 400.
    let no_device = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .json(&BundleInitRequest {
            bundle_id: Uuid::now_v7(),
            device_hint: None,
            sha256: sha.clone(),
            size_bytes: 1,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(no_device.status(), reqwest::StatusCode::BAD_REQUEST);
}

/// Must-fix #1 regression: before the body-limit fix, Axum's 2 MiB default
/// body cap would 413 any chunk over ~2 MiB before the handler ran. We send
/// a 4 MiB chunk (comfortably past the old ceiling, well under the 32 MiB
/// MAX_CHUNK_SIZE) and assert it's accepted end-to-end.
#[tokio::test]
async fn large_chunk_above_axum_default_body_limit_is_accepted() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-BIG-03";
    // 4 MiB deterministic payload. Above Axum's 2 MiB default, below our
    // 32 MiB MAX_CHUNK_SIZE cap.
    let size = 4 * 1024 * 1024;
    let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let sha = sha256_hex(&payload);
    let bundle_id = Uuid::now_v7();

    let init: BundleInitResponse = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha.clone(),
            size_bytes: payload.len() as u64,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let resp = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "4 MiB chunk should not be rejected by body limit; got {}",
        resp.status()
    );

    let fin_resp = client
        .post(format!(
            "{}/v1/ingest/bundles/{}/finalize",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .json(&BundleFinalizeRequest {
            final_sha256: sha.clone(),
        })
        .send()
        .await
        .unwrap();
    assert!(fin_resp.status().is_success());
}
