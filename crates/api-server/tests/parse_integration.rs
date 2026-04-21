//! End-to-end test for parse-on-ingest.
//!
//! Builds a tiny evidence-zip (one manifest + three CMTrace <![LOG[...]LOG]!>
//! lines), ships it through the full init → chunk → finalize flow, then polls
//! `GET /v1/sessions/{id}` until `parse_state != "pending"`. Asserts that the
//! background parse worker flipped parse_state to `"ok"` and persisted one
//! files row + three entries rows with the expected severity values
//! (Info=0, Warning=1, Error=2).
//!
//! The zip is constructed inline rather than pointing at `tools/fixtures/
//! test-bundle.zip` because that artifact is built by a bash script and
//! isn't present in this crate's test context. Keeping the fixture code
//! next to the assertion also makes the test self-contained.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, SqliteMetadataStore};
use common_wire::ingest::{
    content_kind, BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest,
    BundleInitResponse,
};
use common_wire::SessionSummary;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::{sleep, Instant};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

/// Minimal evidence-zip with a single CCM-format log file. Three lines,
/// covering Info / Warning / Error severities (CMTrace type 1 / 2 / 3).
fn build_evidence_zip() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        // Default compression — mirrors what the reference fixture emits.
        let opts: SimpleFileOptions =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zw.start_file("manifest.json", opts).unwrap();
        zw.write_all(br#"{"schemaVersion":1,"bundleKind":"evidence-zip"}"#)
            .unwrap();

        zw.start_file("evidence/logs/test.log", opts).unwrap();
        let log = concat!(
            r#"<![LOG[CMTraceOpen test fixture - line 1]LOG]!><time="00:00:00.000+000" date="01-01-2026" component="test" context="" type="1" thread="1" file="test.cpp:1">"#, "\n",
            r#"<![LOG[CMTraceOpen test fixture - line 2 (warning)]LOG]!><time="00:00:01.000+000" date="01-01-2026" component="test" context="" type="2" thread="1" file="test.cpp:2">"#, "\n",
            r#"<![LOG[CMTraceOpen test fixture - line 3 (error)]LOG]!><time="00:00:02.000+000" date="01-01-2026" component="test" context="" type="3" thread="1" file="test.cpp:3">"#, "\n",
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

struct TestServer {
    base: String,
    meta: Arc<SqliteMetadataStore>,
    _tmp: TempDir,
}

async fn start_server() -> TestServer {
    let tmp = TempDir::new().expect("tempdir");
    let blobs = Arc::new(
        LocalFsBlobStore::new(tmp.path())
            .await
            .expect("blob store"),
    );
    // Keep a typed handle to the SQLite store so the test can query
    // entries/files directly. The router only needs the trait object.
    let meta = Arc::new(
        SqliteMetadataStore::connect(":memory:")
            .await
            .expect("sqlite"),
    );
    let state = AppState::new(meta.clone(), blobs, "127.0.0.1:0".to_string());
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    TestServer {
        base,
        meta,
        _tmp: tmp,
    }
}

#[tokio::test]
async fn evidence_zip_is_parsed_and_entries_landed() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-PARSE-01";
    let payload = build_evidence_zip();
    let sha = sha256_hex(&payload);
    let bundle_id = Uuid::now_v7();

    // ----- init + upload + finalize -----
    let init: BundleInitResponse = client
        .post(format!("{}/v1/ingest/bundles", server.base))
        .header("x-device-id", device_id)
        .json(&BundleInitRequest {
            bundle_id,
            device_hint: None,
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

    let chunk_resp = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert!(chunk_resp.status().is_success());

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

    // Finalize must still return "pending" — parse is background.
    assert_eq!(fin.parse_state, "pending");
    let session_id = fin.session_id;

    // ----- poll until parse completes -----
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_state = String::new();
    while Instant::now() < deadline {
        let one: SessionSummary = client
            .get(format!("{}/v1/sessions/{}", server.base, session_id))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if one.parse_state != "pending" {
            final_state = one.parse_state;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        final_state, "ok",
        "expected parse_state=ok within 5s; got {final_state:?}"
    );

    // ----- assert files + entries tables populated -----
    let pool = server.meta.pool();

    let file_count: i64 = sqlx::query("SELECT COUNT(*) FROM files WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(file_count, 1, "exactly one log file should be recorded");

    let entry_count: i64 = sqlx::query("SELECT COUNT(*) FROM entries WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
    assert!(
        entry_count >= 3,
        "expected at least 3 parsed entries, got {entry_count}"
    );

    // Severity distribution sanity-check: the CCM type fields in the
    // fixture are 1/2/3 which map to Info(0) / Warning(1) / Error(2).
    let sev_rows = sqlx::query(
        "SELECT severity, COUNT(*) AS c FROM entries WHERE session_id = ? GROUP BY severity",
    )
    .bind(session_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    let mut has_info = false;
    let mut has_warn = false;
    let mut has_err = false;
    for r in sev_rows {
        let s: i64 = r.get("severity");
        match s {
            0 => has_info = true,
            1 => has_warn = true,
            2 => has_err = true,
            _ => {}
        }
    }
    assert!(has_info, "expected at least one Info (severity=0) entry");
    assert!(has_warn, "expected at least one Warning (severity=1) entry");
    assert!(has_err, "expected at least one Error (severity=2) entry");
}

#[tokio::test]
async fn raw_file_content_kind_marks_parse_failed() {
    // MVP scope: the parse worker only handles evidence-zip. Uploading a
    // raw-file bundle should still finalize cleanly but the background
    // worker flips parse_state to "failed" with a logged reason. This
    // guards against a regression where an unknown kind silently stays
    // "pending" forever.
    let server = start_server().await;
    let client = reqwest::Client::new();

    let device_id = "WIN-PARSE-02";
    let payload = b"not a zip, just some bytes".to_vec();
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

    let _ = client
        .put(format!(
            "{}/v1/ingest/bundles/{}/chunks?offset=0",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .body(payload.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let fin: BundleFinalizeResponse = client
        .post(format!(
            "{}/v1/ingest/bundles/{}/finalize",
            server.base, init.upload_id
        ))
        .header("x-device-id", device_id)
        .json(&BundleFinalizeRequest {
            final_sha256: sha,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_state = String::new();
    while Instant::now() < deadline {
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
        if one.parse_state != "pending" {
            final_state = one.parse_state;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(final_state, "failed");
}
