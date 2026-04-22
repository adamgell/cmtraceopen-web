//! Storage abstractions for the api-server.
//!
//! The three traits ([`BlobStore`] + [`MetadataStore`] + [`AuditStore`]) let
//! route handlers stay agnostic of where bytes and rows live. MVP ships with a
//! local-filesystem blob store and a SQLite metadata/audit store. Later
//! milestones swap in S3 / Postgres without touching handler code.
//!
//! Error strategy: each trait returns its own [`StorageError`]. Route handlers
//! convert to `AppError` for HTTP responses.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

pub mod blob_fs;
pub mod blob_object_store;
#[cfg(feature = "azure")]
pub mod blob_azure;
pub mod meta_sqlite;
pub mod audit_sqlite;

pub use blob_fs::LocalFsBlobStore;
pub use blob_object_store::ObjectStoreBlobStore;
pub use meta_sqlite::SqliteMetadataStore;
pub use audit_sqlite::AuditSqliteStore;

use std::sync::Arc;

use crate::config::{BlobBackend, Config};

/// Build the right [`BlobStore`] implementation for `config` and return it
/// as a trait-object Arc so callers (the Axum AppState, integration tests)
/// can store it without caring which backend is in use.
///
/// Selection rules:
///   - `BlobBackend::Local` → [`LocalFsBlobStore`] rooted at
///     `config.data_dir`. No network deps.
///   - `BlobBackend::Azure` → [`blob_azure::build`] using the validated
///     `blob_azure_*` fields on `config`. Requires the `azure` cargo
///     feature; without it the factory returns
///     [`crate::config::ConfigError::AzureFeatureMissing`] so the operator
///     gets a clear "rebuild with --features azure" message instead of a
///     compile error mid-deploy.
///
/// This is the *only* place the codebase chooses a backend. Adding S3 / GCS
/// later means: new feature flag in `Cargo.toml`, new `BlobBackend`
/// variant, new arm here, new factory module — handlers and tests don't
/// move.
pub async fn build_blob_store(
    config: &Config,
) -> Result<Arc<dyn BlobStore>, BuildBlobStoreError> {
    match config.blob_backend {
        BlobBackend::Local => {
            let store = LocalFsBlobStore::new(&config.data_dir).await?;
            Ok(Arc::new(store))
        }
        BlobBackend::Azure => build_azure_blob_store(config).await,
    }
}

#[cfg(feature = "azure")]
async fn build_azure_blob_store(
    config: &Config,
) -> Result<Arc<dyn BlobStore>, BuildBlobStoreError> {
    use blob_azure::{AzureAuth, AzureBlobConfig};

    // Config::from_env already validated that both account + container are
    // present and that exactly one auth mode is set when blob_backend ==
    // Azure, so the unwraps below are infallible at this point. We still
    // map errors through BuildBlobStoreError rather than `expect()` so a
    // mis-call from a test that bypasses Config::from_env produces a clean
    // error instead of a panic.
    let account = config
        .blob_azure_account
        .clone()
        .ok_or(BuildBlobStoreError::MissingAzureField("account"))?;
    let container = config
        .blob_azure_container
        .clone()
        .ok_or(BuildBlobStoreError::MissingAzureField("container"))?;

    let auth = if let Some(key) = config.blob_azure_account_key.clone() {
        AzureAuth::AccountKey(key)
    } else if config.blob_azure_use_managed_identity {
        AzureAuth::ManagedIdentity
    } else {
        return Err(BuildBlobStoreError::MissingAzureField("auth"));
    };

    let staging_root = config.data_dir.join("staging");
    let azure_cfg = AzureBlobConfig {
        account_name: account,
        container_name: container,
        staging_root,
        auth,
    };
    let store = blob_azure::build(azure_cfg).await?;
    Ok(Arc::new(store))
}

#[cfg(not(feature = "azure"))]
async fn build_azure_blob_store(
    _config: &Config,
) -> Result<Arc<dyn BlobStore>, BuildBlobStoreError> {
    // The `azure` cargo feature is off but the operator selected Azure at
    // runtime. Bail out loudly rather than silently falling back to local.
    Err(BuildBlobStoreError::AzureFeatureMissing)
}

/// Errors surfaced by [`build_blob_store`]. Distinct from
/// [`crate::config::ConfigError`] so this stays runtime-only — env-time
/// validation and runtime construction can fail differently and we want the
/// operator-facing messages to reflect that.
#[derive(Debug, Error)]
pub enum BuildBlobStoreError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error(
        "blob backend Azure was selected at runtime but a required field is \
         missing from Config: {0}. Most likely Config was built without going \
         through Config::from_env."
    )]
    MissingAzureField(&'static str),

    #[error(
        "blob backend Azure was selected at runtime but the api-server was \
         built without the `azure` cargo feature. Rebuild with \
         `--features azure` (the default)."
    )]
    AzureFeatureMissing,
}

#[cfg(test)]
mod build_blob_store_tests {
    //! Tests for the backend-selection factory. We don't need a full
    //! `Config::from_env` run here; constructing `Config` literals lets the
    //! factory's branching get exercised in isolation. Real network
    //! round-trips against Azure live in
    //! `tests/azure_blob_integration.rs`, gated on
    //! `CMTRACE_AZURE_STORAGE_ACCOUNT` being set in the environment.

    use super::*;
    use crate::auth::AuthMode;
    use crate::config::{BlobBackend, Config, TlsConfig};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn base_config(data_dir: PathBuf) -> Config {
        Config {
            listen_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            data_dir: data_dir.clone(),
            sqlite_path: ":memory:".to_string(),
            auth_mode: AuthMode::Disabled,
            entra: None,
            allowed_origins: vec![],
            allow_credentials: false,
            tls: TlsConfig::default(),
            crl_urls: vec![],
            crl_refresh_secs: 3600,
            crl_fail_open: false,
            blob_backend: BlobBackend::Local,
            blob_azure_account: None,
            blob_azure_container: None,
            blob_azure_account_key: None,
            blob_azure_use_managed_identity: false,
            bundle_ttl_days: 90,
            retention_scan_interval_secs: 21_600,
            retention_batch_size: 100,
        }
    }

    #[tokio::test]
    async fn factory_picks_local_for_default_backend() {
        let tmp = TempDir::new().unwrap();
        let cfg = base_config(tmp.path().to_path_buf());
        let store = build_blob_store(&cfg)
            .await
            .expect("local backend should always build");

        // Smoke-test the trait contract on the returned Arc<dyn BlobStore>:
        // create_staging then discard. If we got the wrong backend back the
        // staging path wouldn't land under the tempdir.
        let upload_id = uuid::Uuid::now_v7();
        store.create_staging(upload_id).await.unwrap();
        let staging = store.staging_path(upload_id);
        assert!(
            staging.starts_with(tmp.path()),
            "staging path {staging:?} should be under tempdir {:?}",
            tmp.path()
        );
        store.discard_staging(upload_id).await.unwrap();
    }

    #[cfg(feature = "azure")]
    #[tokio::test]
    async fn factory_picks_azure_for_azure_backend() {
        // No real network call here — we only verify the factory dispatches
        // to the Azure path and the resulting object_store client builds
        // (the MicrosoftAzureBuilder validation is local).
        let tmp = TempDir::new().unwrap();
        let mut cfg = base_config(tmp.path().to_path_buf());
        cfg.blob_backend = BlobBackend::Azure;
        cfg.blob_azure_account = Some("devstoreaccount1".to_string());
        cfg.blob_azure_container = Some("test-bucket".to_string());
        cfg.blob_azure_account_key = Some(
            "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6\
             IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=="
                .to_string(),
        );

        let store = build_blob_store(&cfg).await.expect("azure backend builds");

        // Confirm staging dir was created at the expected location (the
        // Azure backend still stages locally before shipping to the cloud).
        let upload_id = uuid::Uuid::now_v7();
        store.create_staging(upload_id).await.unwrap();
        let staging = store.staging_path(upload_id);
        assert!(staging.starts_with(tmp.path().join("staging")));
        store.discard_staging(upload_id).await.unwrap();
    }

    #[cfg(not(feature = "azure"))]
    #[tokio::test]
    async fn factory_rejects_azure_when_feature_disabled() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = base_config(tmp.path().to_path_buf());
        cfg.blob_backend = BlobBackend::Azure;
        // `Arc<dyn BlobStore>` doesn't implement Debug, so expect_err
        // doesn't compile here — match the result manually instead.
        match build_blob_store(&cfg).await {
            Ok(_) => panic!("azure backend should fail when feature disabled"),
            Err(BuildBlobStoreError::AzureFeatureMissing) => {}
            Err(other) => panic!("expected AzureFeatureMissing, got {other:?}"),
        }
    }
}

/// Opaque handle returned by [`BlobStore::finalize`]. For the local-fs impl
/// this is a `file://` URI; for a future S3 impl it would be `s3://…`.
#[derive(Debug, Clone)]
pub struct BlobHandle {
    pub uri: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Snapshot of a metadata-store connection pool's health.
///
/// Surfaced on the dev status page (`GET /`) so operators can spot pool
/// starvation at a glance without having to wire up Prometheus. Fields mirror
/// the three sqlx `Pool` getters we care about — total current connections,
/// idle connections, and the configured ceiling. Backends without a real
/// pool (e.g. a future in-memory mock) can return zeros via the trait's
/// default impl.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Connections currently held by the pool (idle + in-use).
    pub size: u32,
    /// Connections currently idle and available for checkout.
    pub idle: u32,
    /// Configured upper bound on `size`.
    pub max_size: u32,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("upload {0} not found")]
    UploadNotFound(Uuid),

    #[error("offset mismatch: expected {expected}, got {actual}")]
    OffsetMismatch { expected: u64, actual: u64 },

    #[error("size overflow: upload would exceed declared size {declared} (got {attempted})")]
    SizeOverflow { declared: u64, attempted: u64 },

    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch { expected: String, actual: String },

    #[error("upload {0} is already finalized")]
    AlreadyFinalized(Uuid),

    #[error("conflict: session for (device_id={device_id}, bundle_id={bundle_id}) already exists")]
    SessionConflict { device_id: String, bundle_id: Uuid },

    /// `object_store` backend surfaced a non-I/O error (auth, network, 4xx
    /// from Azure, etc). Kept as a stringified form so the trait doesn't
    /// leak `object_store::Error` into consumers that might be compiled
    /// without the `azure` feature and its transitive enum variants.
    #[error("object store error: {0}")]
    ObjectStore(String),

    /// [`BlobStore::read_blob`] / [`BlobStore::head_blob`] got a URI that
    /// doesn't match the scheme this store was configured with (e.g. an
    /// `azure://` URI reached a local-FS-configured server after a backend
    /// change). Callers surface this as a 500 — the row in `sessions.blob_uri`
    /// is stale for the current deployment.
    #[error("blob URI not recognized by current backend: {0}")]
    BadBlobUri(String),
}

/// Row-shaped view of the `uploads` table used by the ingest handlers.
#[derive(Debug, Clone)]
pub struct UploadRow {
    pub upload_id: Uuid,
    pub bundle_id: Uuid,
    pub device_id: String,
    pub size_bytes: u64,
    pub expected_sha256: String,
    pub content_kind: String,
    pub offset_bytes: u64,
    pub staged_path: String,
    pub created_utc: DateTime<Utc>,
    pub finalized: bool,
}

/// Row-shaped view of the `sessions` table.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub session_id: Uuid,
    pub device_id: String,
    pub bundle_id: Uuid,
    pub blob_uri: String,
    pub content_kind: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub collected_utc: Option<DateTime<Utc>>,
    pub ingested_utc: DateTime<Utc>,
    pub parse_state: String,
}

/// Row-shaped view of the `devices` table used by the registry queries.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub device_id: String,
    pub first_seen_utc: DateTime<Utc>,
    pub last_seen_utc: DateTime<Utc>,
    pub hostname: Option<String>,
    pub session_count: i64,
}

/// Row-shaped view of the `files` table populated by parse-on-ingest.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub file_id: String,
    pub session_id: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub format_detected: Option<String>,
    pub parser_kind: Option<String>,
    pub entry_count: u64,
    pub parse_error_count: u64,
}

/// Row-shaped view of the `entries` table populated by parse-on-ingest.
#[derive(Debug, Clone)]
pub struct EntryRow {
    pub entry_id: i64,
    pub file_id: String,
    pub line_number: u32,
    pub ts_ms: Option<i64>,
    /// Numeric severity in the DB (0/1/2). Rendered to string at the wire.
    pub severity: i64,
    pub component: Option<String>,
    pub thread: Option<String>,
    pub message: String,
    /// Raw JSON text from `entries.extras_json`. Route handlers parse it
    /// into a `serde_json::Value` for the DTO — keeping it as a string here
    /// lets the storage layer stay json-library-free.
    pub extras_json: Option<String>,
}

/// Filters applied by the entries-query route.
///
/// Any combination may be set. Semantics:
///   - `file_id`: restrict to a single file_id.
///   - `min_severity`: numeric floor on `entries.severity` (inclusive).
///   - `after_ts_ms`: entries with `ts_ms >= after_ts_ms` (inclusive). Rows
///     with NULL `ts_ms` are excluded when this bound is set.
///   - `before_ts_ms`: entries with `ts_ms < before_ts_ms` (exclusive).
///     Rows with NULL `ts_ms` are excluded when this bound is set.
///   - `q_like`: plain substring filter; the caller builds the `%…%`
///     wrapping so `LIKE` semantics (incl. escape handling) live in one
///     place.
///
/// `cursor` carries the keyset position: the `(ts_ms, entry_id)` pair of the
/// last row returned on the previous page. `None` means "start from the
/// top."
#[derive(Debug, Clone, Default)]
pub struct EntryFilters {
    pub file_id: Option<String>,
    pub min_severity: Option<i64>,
    pub after_ts_ms: Option<i64>,
    pub before_ts_ms: Option<i64>,
    pub q_like: Option<String>,
    pub cursor: Option<EntryCursor>,
}

/// Keyset cursor over `ORDER BY ts_ms NULLS LAST, entry_id ASC`.
///
/// We store `ts_ms` as `Option<i64>` verbatim so the "NULLS LAST" tier is
/// representable: a `None` cursor means the previous page ended on a
/// NULL-timestamp row and the next page should continue among the
/// NULL-timestamp tail, ordered by `entry_id`.
#[derive(Debug, Clone)]
pub struct EntryCursor {
    pub ts_ms: Option<i64>,
    pub entry_id: i64,
}

/// Parameters for creating a new upload session.
#[derive(Debug, Clone)]
pub struct NewUpload {
    pub upload_id: Uuid,
    pub bundle_id: Uuid,
    pub device_id: String,
    pub size_bytes: u64,
    pub expected_sha256: String,
    pub content_kind: String,
    pub staged_path: String,
}

/// A logical file discovered inside a bundle and fed through the parser.
///
/// Used as the FK parent for [`NewEntry`] rows so a failing parse can be
/// recorded alongside whatever entries did land — callers allocate `file_id`
/// (UUID v7) up front so the entries can be linked before commit.
#[derive(Debug, Clone)]
pub struct NewFile {
    pub file_id: Uuid,
    pub session_id: Uuid,
    pub relative_path: String,
    pub size_bytes: u64,
    pub format_detected: Option<String>,
    pub parser_kind: Option<String>,
    pub entry_count: u32,
    pub parse_error_count: u32,
}

/// A single parsed log entry destined for the `entries` table.
///
/// Severity is the numeric int form (0=Info/1=Warning/2=Error) the column
/// stores — mapping from the parser's `Severity` enum happens in
/// `pipeline::parse_worker`, so the storage layer doesn't take a parser dep.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub session_id: Uuid,
    pub file_id: Uuid,
    pub line_number: u32,
    pub ts_ms: Option<i64>,
    pub severity: i32,
    pub component: Option<String>,
    pub thread: Option<String>,
    pub message: String,
    pub extras_json: Option<String>,
}

/// Content-addressed + session-keyed blob storage.
#[async_trait]
pub trait BlobStore: Send + Sync + 'static {
    /// Path where chunks for an in-progress upload are staged. Returned so
    /// handlers can tell the metadata store where the file lives without the
    /// blob store owning DB state.
    fn staging_path(&self, upload_id: Uuid) -> std::path::PathBuf;

    /// Create an empty staging file for a new upload.
    async fn create_staging(&self, upload_id: Uuid) -> Result<(), StorageError>;

    /// Append a chunk at `offset`. The caller must have already verified the
    /// offset matches the server cursor.
    async fn put_chunk(
        &self,
        upload_id: Uuid,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), StorageError>;

    /// Compute sha256 over the fully-assembled staging file.
    async fn hash(&self, upload_id: Uuid) -> Result<String, StorageError>;

    /// Move the staging file to its final blob location keyed by
    /// `session_id`. Returns a handle the caller stores in `sessions.blob_uri`.
    async fn finalize(
        &self,
        upload_id: Uuid,
        session_id: Uuid,
    ) -> Result<BlobHandle, StorageError>;

    /// Best-effort cleanup of a staging file (e.g. after a sha256 mismatch).
    async fn discard_staging(&self, upload_id: Uuid) -> Result<(), StorageError>;

    /// Return the size of a finalized blob (in bytes). Used by the parse
    /// worker to enforce its in-memory unzip cap before pulling the bytes
    /// down — for cloud backends this saves us from streaming a multi-GB
    /// bundle just to find out it's too big.
    async fn head_blob(&self, uri: &str) -> Result<u64, StorageError>;

    /// Read a finalized blob into memory. The parse worker calls this with
    /// the URI returned by [`Self::finalize`]. Implementations MUST round-
    /// trip the URIs they themselves emit; URIs produced by a different
    /// backend should fail as [`StorageError::BadBlobUri`].
    ///
    /// Returns `Vec<u8>` rather than a stream because the only consumer
    /// today is the in-memory zip walker — adding a streaming variant is
    /// straightforward when an `ndjson-entries` parser lands.
    async fn read_blob(&self, uri: &str) -> Result<Vec<u8>, StorageError>;

    /// Hard-delete a finalized blob. Used by the retention sweeper (see
    /// `pipeline::retention`) to free storage when a session ages past the
    /// configured TTL.
    ///
    /// Implementations MUST treat "blob not found" as success — the
    /// sweeper is idempotent by design: if a previous run already removed
    /// the blob but crashed before clearing the metadata row, the next
    /// scan re-issues the delete and expects it to be a no-op rather than
    /// an error. Unrecognized URI schemes still return
    /// [`StorageError::BadBlobUri`] (same contract as `read_blob` /
    /// `head_blob`).
    async fn delete_blob(&self, uri: &str) -> Result<(), StorageError>;
}

/// Relational metadata operations. Split out so handlers can be unit-tested
/// against an in-memory SQLite or a future mock without mocking HTTP.
#[async_trait]
pub trait MetadataStore: Send + Sync + 'static {
    /// Snapshot of the underlying connection pool's health.
    ///
    /// Used by the dev status page (`GET /`) to render pool utilization
    /// without the route handler reaching into a backend-specific pool type.
    /// The default impl returns all zeros so non-pooled backends (mocks,
    /// future in-memory stores) don't have to care.
    fn pool_stats(&self) -> PoolStats {
        PoolStats::default()
    }

    // ----- devices -----

    /// Upsert a device row: inserts on first-seen; updates `last_seen_utc`
    /// (and optionally `hostname`) on subsequent ingests.
    async fn upsert_device(
        &self,
        device_id: &str,
        hostname: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError>;

    async fn list_devices(
        &self,
        limit: u32,
        after_device_id: Option<&str>,
    ) -> Result<Vec<DeviceRow>, StorageError>;

    // ----- uploads -----

    async fn insert_upload(&self, new: NewUpload, now: DateTime<Utc>) -> Result<(), StorageError>;

    async fn get_upload(&self, upload_id: Uuid) -> Result<UploadRow, StorageError>;

    /// Advance an upload's cursor. Called after a successful chunk write.
    async fn set_upload_offset(
        &self,
        upload_id: Uuid,
        new_offset: u64,
    ) -> Result<(), StorageError>;

    /// Atomically advance the cursor iff it currently equals
    /// `expected_offset`. Returns `Ok(true)` when exactly one row updated,
    /// `Ok(false)` when the row exists but the cursor had already moved
    /// (i.e. a concurrent PUT won the race), and
    /// `Err(UploadNotFound)` when the upload_id doesn't exist at all.
    ///
    /// Callers use this to make the "read offset → write offset" sequence
    /// in the chunk handler atomic at the DB level, closing the
    /// time-of-check/time-of-use race between two concurrent PUTs at the
    /// same offset.
    async fn compare_and_set_upload_offset(
        &self,
        upload_id: Uuid,
        expected_offset: u64,
        new_offset: u64,
    ) -> Result<bool, StorageError>;

    async fn mark_upload_finalized(&self, upload_id: Uuid) -> Result<(), StorageError>;

    /// Look up an existing upload for (device_id, bundle_id) that we can
    /// resume. Returns None if no prior interrupted upload exists.
    async fn find_resumable_upload(
        &self,
        device_id: &str,
        bundle_id: Uuid,
    ) -> Result<Option<UploadRow>, StorageError>;

    // ----- sessions -----

    async fn insert_session(&self, row: SessionRow) -> Result<(), StorageError>;

    /// Return Some(session) if (device_id, bundle_id) already has a finalized
    /// session. Used to short-circuit duplicate finalize calls.
    async fn find_session_by_bundle(
        &self,
        device_id: &str,
        bundle_id: Uuid,
    ) -> Result<Option<SessionRow>, StorageError>;

    async fn get_session(&self, session_id: Uuid) -> Result<Option<SessionRow>, StorageError>;

    async fn list_sessions_for_device(
        &self,
        device_id: &str,
        limit: u32,
        before: Option<(DateTime<Utc>, Uuid)>,
    ) -> Result<Vec<SessionRow>, StorageError>;

    /// Most recently ingested sessions across **all** devices, ordered by
    /// `ingested_utc DESC`. Used by the dev status page (`GET /`) to surface
    /// a "recent bundles" panel without the operator having to know a
    /// specific device id up front. Returns up to `limit` rows.
    async fn recent_sessions(&self, limit: u32) -> Result<Vec<SessionRow>, StorageError>;

    /// Flip `sessions.parse_state` to `ok` / `partial` / `failed` after the
    /// background parse worker finishes. Any other value is accepted
    /// verbatim so callers can add granular states later (e.g. `timeout`)
    /// without a schema migration; the DB column is `TEXT`.
    async fn update_session_parse_state(
        &self,
        session_id: Uuid,
        state: &str,
    ) -> Result<(), StorageError>;

    // ----- parsed-files + entries -----

    /// Insert a `files` row and return its `file_id` so the caller can fan
    /// parsed entries into `insert_entries_batch` under the same FK.
    ///
    /// The caller allocates `file_id` (UUID v7) up front — this keeps the
    /// trait SQLite-agnostic and lets a future Postgres backend use the same
    /// insert without `RETURNING`.
    async fn insert_file(&self, new: NewFile) -> Result<Uuid, StorageError>;

    /// Bulk-insert parsed entries for one session in a single transaction.
    ///
    /// Wrapping in a transaction means a mid-batch failure can't leave the
    /// `entries` table half-populated — the worker catches the error and
    /// flips `parse_state` to `failed` cleanly. Empty input is a no-op.
    async fn insert_entries_batch(&self, entries: Vec<NewEntry>) -> Result<(), StorageError>;

    // ----- files / entries query side (entries-query route) -----

    /// List files belonging to a session. Keyset-paginated on `file_id`
    /// ascending — UUIDv7 is time-sortable so this yields insertion order
    /// without a dedicated created_utc column.
    async fn list_files_for_session(
        &self,
        session_id: Uuid,
        limit: u32,
        after_file_id: Option<&str>,
    ) -> Result<Vec<FileRow>, StorageError>;

    /// Query parsed log entries for a session. Callers build [`EntryFilters`]
    /// from the HTTP query string; this method returns one page plus the
    /// cursor for the next page baked into the last row.
    async fn query_entries(
        &self,
        session_id: Uuid,
        filters: &EntryFilters,
        limit: u32,
    ) -> Result<Vec<EntryRow>, StorageError>;

    // ----- retention -----

    /// Return up to `batch_size` sessions whose `ingested_utc` is older than
    /// `ttl_days` ago, ordered by `ingested_utc ASC` (oldest first).
    ///
    /// Each row carries the `session_id` + the `blob_uri` so the retention
    /// sweeper can issue a `BlobStore::delete_blob` before clearing the
    /// metadata row. A `ttl_days` of zero is reserved by the caller as
    /// "never sweep" and should never reach this method — callers gate on
    /// it before invoking.
    async fn sessions_older_than(
        &self,
        ttl_days: u32,
        batch_size: u32,
    ) -> Result<Vec<(Uuid, String)>, StorageError>;

    /// Delete a session row plus its parse-on-ingest fan-out (`files` and
    /// `entries`). Wrapped in a single transaction at the SQL layer so a
    /// crash mid-delete leaves the session either fully present or fully
    /// gone; callers (the retention sweeper) tolerate the small window
    /// where the blob has been removed but the metadata row is still
    /// present (next scan re-tries the cycle).
    ///
    /// Returns the number of `entries` rows deleted so the sweeper can
    /// surface the cleanup volume in its summary log line. Missing
    /// sessions return `Ok(0)` rather than an error — same idempotency
    /// rationale as `BlobStore::delete_blob`.
    async fn delete_session(&self, session_id: Uuid) -> Result<u64, StorageError>;
}

// ===========================================================================
// Audit log
// ===========================================================================

/// A row in the `audit_log` table, as returned by [`AuditStore::list_audit_rows`].
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: Uuid,
    pub ts_utc: DateTime<Utc>,
    pub principal_kind: String,
    pub principal_id: String,
    pub principal_display: Option<String>,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub result: String,
    pub details_json: Option<String>,
    pub request_id: Option<Uuid>,
}

/// Parameters for inserting a new audit row.
#[derive(Debug, Clone)]
pub struct NewAuditRow {
    pub id: Uuid,
    pub ts_utc: DateTime<Utc>,
    pub principal_kind: String,
    pub principal_id: String,
    pub principal_display: Option<String>,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub result: String,
    pub details_json: Option<String>,
    pub request_id: Option<Uuid>,
}

/// Optional filters for [`AuditStore::list_audit_rows`].
///
/// All fields default to `None`, meaning "no filter applied".
#[derive(Debug, Clone, Default)]
pub struct AuditFilters {
    /// Include only rows with `ts_utc > after_ts` (exclusive lower bound).
    pub after_ts: Option<DateTime<Utc>>,
    /// Include only rows whose `principal_id` equals this value.
    pub principal: Option<String>,
    /// Include only rows whose `action` equals this value.
    pub action: Option<String>,
}

/// Append-only audit log. Implementations MUST never update or delete rows —
/// the table is a tamper-evident record of admin/operator actions.
#[async_trait]
pub trait AuditStore: Send + Sync + 'static {
    /// Append one audit row. Called by the audit middleware after every
    /// auditable admin request (both successful and failed).
    async fn insert_audit_row(&self, row: NewAuditRow) -> Result<(), StorageError>;

    /// Page through the audit log in reverse-chronological order.
    ///
    /// Results are ordered `ts_utc DESC`. `limit` is clamped to a
    /// backend-defined maximum (typically 1000). Callers apply
    /// [`AuditFilters`] to narrow the result set.
    async fn list_audit_rows(
        &self,
        filters: &AuditFilters,
        limit: u32,
    ) -> Result<Vec<AuditRow>, StorageError>;
}

/// No-op [`AuditStore`] used in tests that don't exercise the audit surface.
///
/// `insert_audit_row` silently succeeds; `list_audit_rows` always returns an
/// empty list. This lets existing `AppState` constructors compile unchanged —
/// callers that care about audit (the integration tests, production `main.rs`)
/// swap in a real backend via [`AppState::full`] / [`AppState::with_cors_and_crl`].
pub struct NoopAuditStore;

#[async_trait]
impl AuditStore for NoopAuditStore {
    async fn insert_audit_row(&self, _row: NewAuditRow) -> Result<(), StorageError> {
        Ok(())
    }

    async fn list_audit_rows(
        &self,
        _filters: &AuditFilters,
        _limit: u32,
    ) -> Result<Vec<AuditRow>, StorageError> {
        Ok(vec![])
    }
}
