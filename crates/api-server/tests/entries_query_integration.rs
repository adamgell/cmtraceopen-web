//! End-to-end integration tests for the per-session files + entries query
//! routes added in `feat/entries-query-route`.
//!
//! Sister-PR caveat: in the merged stack the parse-on-ingest branch will
//! populate `files` + `entries` automatically when a bundle finalize runs.
//! That parser isn't on this branch, so this test seeds the rows directly
//! through the SQLite pool and then exercises the HTTP surface.
//!
//! Coverage:
//!   - GET /v1/sessions/{id}/files returns a single FileSummary.
//!   - GET /v1/sessions/{id}/entries returns ≥3 entries.
//!   - Walking with `limit=1` paginates through every entry exactly once.
//!   - `severity=warning` filter excludes Info-only rows.
//!   - Unknown session_id returns 404.
//!   - `limit > 500` returns 400.

use std::sync::Arc;

use api_server::router;
use api_server::state::AppState;
use api_server::storage::{LocalFsBlobStore, MetadataStore, SessionRow, SqliteMetadataStore};
use chrono::Utc;
use common_wire::{FileSummary, LogEntryDto, Paginated};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::net::TcpListener;
use uuid::Uuid;

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
    let meta = Arc::new(
        SqliteMetadataStore::connect(":memory:")
            .await
            .expect("sqlite"),
    );
    let state = AppState::new_auth_disabled(meta.clone(), blobs, "127.0.0.1:0".to_string());
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

/// Seed a device + session so the route's existence check passes, then
/// insert one file + N entries directly. Mirrors what parse-on-ingest will
/// do at finalize time.
async fn seed_session_with_entries(
    server: &TestServer,
    device_id: &str,
) -> (Uuid, String) {
    let now = Utc::now();
    server
        .meta
        .upsert_device(device_id, Some("lab"), now)
        .await
        .unwrap();

    let session_id = Uuid::now_v7();
    let bundle_id = Uuid::now_v7();
    server
        .meta
        .insert_session(SessionRow {
            session_id,
            device_id: device_id.to_string(),
            bundle_id,
            blob_uri: "file:///tmp/x".to_string(),
            content_kind: "evidence-zip".to_string(),
            size_bytes: 0,
            sha256: "0".repeat(64),
            collected_utc: None,
            ingested_utc: now,
            parse_state: "ok".to_string(),
        })
        .await
        .unwrap();

    let file_id = Uuid::now_v7().to_string();
    let pool: &SqlitePool = server.meta.pool();
    sqlx::query(
        r#"
        INSERT INTO files
          (file_id, session_id, relative_path, size_bytes,
           format_detected, parser_kind, entry_count, parse_error_count)
        VALUES (?, ?, ?, ?, ?, ?, ?, 0)
        "#,
    )
    .bind(&file_id)
    .bind(session_id.to_string())
    .bind("ccmexec.log")
    .bind(1234_i64)
    .bind("cmtrace")
    .bind("cmtrace")
    .bind(4_i64)
    .execute(pool)
    .await
    .unwrap();

    // Four entries: I, W, E, and an extra Info — also includes a
    // null-timestamp row to exercise the NULLS-LAST cursor tier.
    let rows = [
        (Some(1_700_000_000_000_i64), 0, "first info"),
        (Some(1_700_000_000_500_i64), 1, "warning at 500"),
        (Some(1_700_000_001_000_i64), 2, "boom error"),
        (None, 0, "tail entry with no timestamp"),
    ];
    for (line_no, (ts, sev, msg)) in rows.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO entries
              (file_id, session_id, line_number, ts_ms, severity,
               component, thread, message, extras_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&file_id)
        .bind(session_id.to_string())
        .bind((line_no + 1) as i64)
        .bind(*ts)
        .bind(*sev as i64)
        .bind(Some("ccmexec"))
        .bind::<Option<&str>>(None)
        .bind(*msg)
        .bind(Some(r#"{"k":"v"}"#))
        .execute(pool)
        .await
        .unwrap();
    }

    (session_id, file_id)
}

#[tokio::test]
async fn list_files_returns_seeded_file() {
    let server = start_server().await;
    let (session_id, file_id) = seed_session_with_entries(&server, "WIN-FQ-01").await;

    let client = reqwest::Client::new();
    let page: Paginated<FileSummary> = client
        .get(format!("{}/v1/sessions/{}/files", server.base, session_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].file_id, file_id);
    assert_eq!(page.items[0].relative_path, "ccmexec.log");
    assert_eq!(page.items[0].entry_count, 4);
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn list_entries_returns_all_then_paginates_with_limit_one() {
    let server = start_server().await;
    let (session_id, _) = seed_session_with_entries(&server, "WIN-FQ-02").await;

    let client = reqwest::Client::new();

    // Default page returns all four entries.
    let page: Paginated<LogEntryDto> = client
        .get(format!("{}/v1/sessions/{}/entries", server.base, session_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.items.len(), 4, "all seeded entries should come back");
    assert!(page.next_cursor.is_none());
    // Ordering: ts_ms ASC then NULL last.
    assert_eq!(page.items[0].message, "first info");
    assert_eq!(page.items[1].message, "warning at 500");
    assert_eq!(page.items[2].message, "boom error");
    assert_eq!(page.items[3].message, "tail entry with no timestamp");
    // Severity rendered as canonical string.
    assert_eq!(page.items[0].severity, "Info");
    assert_eq!(page.items[1].severity, "Warning");
    assert_eq!(page.items[2].severity, "Error");
    // Extras decoded into a JSON object.
    assert_eq!(page.items[0].extras.as_ref().unwrap()["k"], "v");

    // Walk with limit=1 — should yield exactly four pages then stop.
    let mut cursor: Option<String> = None;
    let mut seen: Vec<String> = Vec::new();
    for step in 0..10 {
        let mut url = format!("{}/v1/sessions/{}/entries?limit=1", server.base, session_id);
        if let Some(ref c) = cursor {
            url.push_str("&cursor=");
            url.push_str(c);
        }
        let p: Paginated<LogEntryDto> = client
            .get(&url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if p.items.is_empty() {
            break;
        }
        assert_eq!(p.items.len(), 1, "limit=1 should yield one row at step {step}");
        seen.push(p.items[0].message.clone());
        match p.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 4, "limit=1 walk should visit every entry");
    assert_eq!(seen[0], "first info");
    assert_eq!(seen[3], "tail entry with no timestamp");
}

#[tokio::test]
async fn severity_filter_drops_info_rows() {
    let server = start_server().await;
    let (session_id, _) = seed_session_with_entries(&server, "WIN-FQ-03").await;
    let client = reqwest::Client::new();

    let page: Paginated<LogEntryDto> = client
        .get(format!(
            "{}/v1/sessions/{}/entries?severity=warning",
            server.base, session_id
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    // Two non-info rows in the seed: warning + error.
    assert_eq!(page.items.len(), 2);
    assert!(page.items.iter().all(|e| e.severity != "Info"));

    let errors_only: Paginated<LogEntryDto> = client
        .get(format!(
            "{}/v1/sessions/{}/entries?severity=error",
            server.base, session_id
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(errors_only.items.len(), 1);
    assert_eq!(errors_only.items[0].severity, "Error");
}

#[tokio::test]
async fn unknown_session_returns_404() {
    let server = start_server().await;
    let client = reqwest::Client::new();
    let bogus = Uuid::now_v7();

    let r = client
        .get(format!("{}/v1/sessions/{}/files", server.base, bogus))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);

    let r = client
        .get(format!("{}/v1/sessions/{}/entries", server.base, bogus))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn limit_above_max_is_rejected() {
    let server = start_server().await;
    let (session_id, _) = seed_session_with_entries(&server, "WIN-FQ-04").await;
    let client = reqwest::Client::new();

    let r = client
        .get(format!(
            "{}/v1/sessions/{}/entries?limit=10000",
            server.base, session_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
}
