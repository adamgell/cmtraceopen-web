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
    let state = AppState::new(meta, blobs, "127.0.0.1:0".to_string());
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

/// Must-fix #2 regression: re-initializing the same `bundle_id` with a
/// different `sha256` (or size / contentKind) while a prior upload is still
/// open must 409 rather than silently returning the stale upload_id. That
/// would let a retry mix bytes from a completely different bundle.
#[tokio::test]
async fn resume_with_drifted_init_invariants_is_rejected() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-DRIFT-04";
    let bundle_id = Uuid::now_v7();

    // First init: sha of "aaaa".
    let payload_a = b"aaaa".to_vec();
    let sha_a = sha256_hex(&payload_a);
    let init_a = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha_a.clone(),
            size_bytes: payload_a.len() as u64,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap();
    assert!(init_a.status().is_success(), "first init: {}", init_a.status());

    // Second init, same bundle_id, different sha256 → 409.
    let payload_b = b"bbbb".to_vec();
    let sha_b = sha256_hex(&payload_b);
    let init_b_sha = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha_b.clone(),
            size_bytes: payload_a.len() as u64,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(
        init_b_sha.status(),
        reqwest::StatusCode::CONFLICT,
        "expected 409 on sha drift"
    );
    let body: serde_json::Value = init_b_sha.json().await.unwrap();
    assert!(
        body["message"].as_str().unwrap_or("").contains("sha256"),
        "error body should name the drifted field: {body}"
    );

    // Second init, same bundle_id + sha, different sizeBytes → 409.
    let init_b_size = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha_a.clone(),
            size_bytes: (payload_a.len() as u64) + 1,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(
        init_b_size.status(),
        reqwest::StatusCode::CONFLICT,
        "expected 409 on size drift"
    );

    // Second init, same bundle_id + sha + size, different contentKind → 409.
    let init_b_kind = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha_a.clone(),
            size_bytes: payload_a.len() as u64,
            content_kind: content_kind::EVIDENCE_ZIP.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(
        init_b_kind.status(),
        reqwest::StatusCode::CONFLICT,
        "expected 409 on contentKind drift"
    );

    // Identical re-init returns a normal 200 (resume).
    let init_again = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
            sha256: sha_a.clone(),
            size_bytes: payload_a.len() as u64,
            content_kind: content_kind::RAW_FILE.into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(init_again.status(), reqwest::StatusCode::OK);
    let resume: BundleInitResponse = init_again.json().await.unwrap();
    assert_eq!(resume.resume_offset, 0);
}

/// Must-fix #3 regression: the atomic compare-and-set on offset_bytes means
/// exactly one of two concurrent PUTs at the same offset wins. The other
/// gets a 409 offset_mismatch. Without the CAS, both could pass the
/// read-then-write check and double-write.
#[tokio::test]
async fn concurrent_chunks_at_same_offset_one_wins() {
    let server = start_server().await;
    let base = server.base.clone();
    let client = reqwest::Client::new();

    let device_id = "WIN-RACE-05";
    let payload: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let sha = sha256_hex(&payload);
    let bundle_id = Uuid::now_v7();

    let init: BundleInitResponse = client
        .post(format!("{base}/v1/ingest/bundles"))
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

    // Fire two PUTs at offset=0 concurrently.
    let upload_id = init.upload_id;
    let base1 = base.clone();
    let body1 = payload.clone();
    let h1 = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .put(format!(
                    "{base1}/v1/ingest/bundles/{upload_id}/chunks?offset=0"
                ))
                .header("x-device-id", device_id)
                .body(body1)
                .send()
                .await
                .unwrap()
                .status()
        }
    });
    let base2 = base.clone();
    let body2 = payload.clone();
    let h2 = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .put(format!(
                    "{base2}/v1/ingest/bundles/{upload_id}/chunks?offset=0"
                ))
                .header("x-device-id", device_id)
                .body(body2)
                .send()
                .await
                .unwrap()
                .status()
        }
    });

    let s1 = h1.await.unwrap();
    let s2 = h2.await.unwrap();

    let successes = [s1, s2].iter().filter(|s| s.is_success()).count();
    let conflicts = [s1, s2]
        .iter()
        .filter(|s| **s == reqwest::StatusCode::CONFLICT)
        .count();
    assert_eq!(
        successes, 1,
        "exactly one PUT should win (got s1={s1}, s2={s2})"
    );
    assert_eq!(
        conflicts, 1,
        "the loser must 409 offset_mismatch (got s1={s1}, s2={s2})"
    );
}
