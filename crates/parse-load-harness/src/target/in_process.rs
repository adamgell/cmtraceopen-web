//! In-process target — drives `parse_session` directly via the api-server's
//! storage traits, bypassing HTTP entirely. Highest-throughput backend.
//!
//! ## What it does per bundle
//!
//! 1. Stages the zip bytes in the local-FS blob store (tempdir-backed).
//! 2. Finalizes the blob to get a content-addressed URI.
//! 3. Inserts a `sessions` row with `parse_state = "pending"` (mirrors the
//!    ingest finalize route).
//! 4. Calls `parse_worker::parse_session` **directly and awaits it** — there
//!    is no HTTP round-trip so there is nothing to fire-and-forget.
//! 5. Reads back the final `parse_state` from the metadata store.
//!
//! ## Notes
//!
//! * `finalize_ms` is always `None` in the returned `BundleResult` because
//!   there is no HTTP layer.
//! * `files_with_fallbacks` is not currently tracked (requires a separate
//!   query against the `files` table after parse; omitted for MVP).
//! * `shutdown` is a no-op — tempdir cleanup is RAII via `TempDir`'s `Drop`.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use async_trait::async_trait;
use chrono::Utc;
use sysinfo::System;
use tempfile::TempDir;
use uuid::Uuid;

use api_server::pipeline::parse_worker::{self, ParseDeps};
use api_server::storage::{BlobStore, LocalFsBlobStore, MetadataStore, PgMetadataStore, SessionRow};

use crate::bundle::{Bundle, BundleResult};
use crate::target::Target;

pub struct InProcessTarget {
    meta: Arc<dyn MetadataStore>,
    blobs: Arc<dyn BlobStore>,
    /// Kept alive so the tempdir isn't deleted until this struct is dropped.
    _blob_dir: TempDir,
}

impl InProcessTarget {
    /// Build an in-process target backed by a Postgres metadata store at
    /// `pg_url` and a tempdir-backed local-FS blob store.
    pub async fn new(pg_url: String) -> anyhow::Result<Self> {
        let blob_dir = TempDir::new().context("create tempdir for blob store")?;

        let meta = PgMetadataStore::connect(&pg_url)
            .await
            .context("connect PgMetadataStore")?;
        let meta: Arc<dyn MetadataStore> = Arc::new(meta);

        let blobs = LocalFsBlobStore::new(blob_dir.path())
            .await
            .context("create LocalFsBlobStore")?;
        let blobs: Arc<dyn BlobStore> = Arc::new(blobs);

        Ok(Self {
            meta,
            blobs,
            _blob_dir: blob_dir,
        })
    }

    /// Variant for testing without Postgres: accepts any pre-built
    /// `MetadataStore` and uses a tempdir blob store.
    ///
    /// This constructor is only used in integration tests; the real harness
    /// driver calls [`Self::new`].
    #[allow(dead_code)]
    pub async fn with_meta(meta: Arc<dyn MetadataStore>) -> anyhow::Result<Self> {
        let blob_dir = TempDir::new().context("create tempdir for blob store")?;
        let blobs = LocalFsBlobStore::new(blob_dir.path())
            .await
            .context("create LocalFsBlobStore")?;
        let blobs: Arc<dyn BlobStore> = Arc::new(blobs);

        Ok(Self {
            meta,
            blobs,
            _blob_dir: blob_dir,
        })
    }
}

#[async_trait]
impl Target for InProcessTarget {
    /// Drive one full parse cycle for `bundle` in-process.
    ///
    /// Steps:
    ///   1. Stage bytes → `create_staging` + `put_chunk`.
    ///   2. Compute sha256 hash (stored in `sessions.sha256`).
    ///   3. `finalize` → get `BlobHandle` with URI.
    ///   4. Upsert device + insert session row (`parse_state = "pending"`).
    ///   5. `await parse_worker::parse_session` (blocking until done).
    ///   6. Re-read the session to obtain the terminal `parse_state`.
    async fn send(&self, bundle: Bundle) -> BundleResult {
        let seq = bundle.seq;
        let device_id = bundle.device_id.clone();
        let n_files = bundle.n_files;
        let bundle_bytes = bundle.zip_bytes.len() as u64;

        // Outer timer: error-arm fallback only. On success, parse_ms returned
        // from send_inner is the parse-phase duration after staging.
        let total_start = Instant::now();

        match self.send_inner(bundle).await {
            Ok((parse_state, parse_ms)) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: None, // no HTTP layer
                parse_ms,
                parse_state,
                files_with_fallbacks: None,
                http_status: None,
                error: None,
            },
            Err(e) => BundleResult {
                seq,
                device: device_id,
                n_files,
                bundle_bytes,
                finalize_ms: None,
                parse_ms: total_start.elapsed().as_millis() as u64,
                parse_state: "failed".to_string(),
                files_with_fallbacks: None,
                http_status: None,
                error: Some(format!("{e:#}")),
            },
        }
    }

    /// Sample in-process RSS via `sysinfo`.
    async fn sample_system(&self) -> serde_json::Value {
        let mut sys = System::new();
        sys.refresh_memory();
        let pid = sysinfo::get_current_pid().ok();
        let rss_bytes = pid
            .and_then(|pid| {
                sys.refresh_processes(
                    sysinfo::ProcessesToUpdate::Some(&[pid]),
                    /* remove_dead */ false,
                );
                sys.process(pid).map(|p| p.memory())
            })
            .unwrap_or(0);

        serde_json::json!({
            "rss_bytes": rss_bytes,
            "total_memory_bytes": sys.total_memory(),
            "used_memory_bytes": sys.used_memory(),
        })
    }

    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        // TempDir drops on self drop — nothing else to do.
        Ok(())
    }
}

impl InProcessTarget {
    async fn send_inner(&self, bundle: Bundle) -> anyhow::Result<(String, u64)> {
        let upload_id = Uuid::now_v7();
        let session_id = Uuid::now_v7();
        let now = Utc::now();

        // 1. Stage the bytes.
        self.blobs
            .create_staging(upload_id)
            .await
            .context("create_staging")?;
        self.blobs
            .put_chunk(upload_id, 0, &bundle.zip_bytes)
            .await
            .context("put_chunk")?;

        // 2. Hash.
        let sha256 = self.blobs.hash(upload_id).await.context("hash")?;

        // 3. Finalize blob → URI.
        let handle = self
            .blobs
            .finalize(upload_id, session_id)
            .await
            .context("finalize blob")?;

        // 4. Upsert device + insert session row.
        self.meta
            .upsert_device(&bundle.device_id, None, now)
            .await
            .context("upsert_device")?;

        let row = SessionRow {
            session_id,
            device_id: bundle.device_id.clone(),
            bundle_id: bundle.bundle_id,
            blob_uri: handle.uri.clone(),
            content_kind: "evidence-zip".to_string(),
            size_bytes: handle.size_bytes,
            sha256,
            collected_utc: None,
            ingested_utc: now,
            parse_state: "pending".to_string(),
        };
        self.meta
            .insert_session(row)
            .await
            .context("insert_session")?;

        // 5. Run the parse worker synchronously (no fire-and-forget here).
        let parse_start = Instant::now();
        let deps = ParseDeps {
            meta: self.meta.clone(),
            blobs: self.blobs.clone(),
        };
        parse_worker::parse_session(
            session_id,
            handle.uri,
            "evidence-zip".to_string(),
            deps,
        )
        .await;
        let parse_ms = parse_start.elapsed().as_millis() as u64;

        // 6. Re-read the terminal parse_state.
        let parse_state = self
            .meta
            .get_session(session_id)
            .await
            .context("get_session")?
            .map(|s| s.parse_state)
            .unwrap_or_else(|| "unknown".to_string());

        Ok((parse_state, parse_ms))
    }
}
