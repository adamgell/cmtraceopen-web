//! Storage abstractions for the api-server.
//!
//! The two traits ([`BlobStore`] + [`MetadataStore`]) let route handlers stay
//! agnostic of where bytes and rows live. MVP ships with a local-filesystem
//! blob store and a SQLite metadata store. Later milestones swap in S3 /
//! Postgres without touching handler code.
//!
//! Error strategy: each trait returns its own [`StorageError`]. Route handlers
//! convert to `AppError` for HTTP responses.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

pub mod blob_fs;
pub mod meta_sqlite;

pub use blob_fs::LocalFsBlobStore;
pub use meta_sqlite::SqliteMetadataStore;

/// Opaque handle returned by [`BlobStore::finalize`]. For the local-fs impl
/// this is a `file://` URI; for a future S3 impl it would be `s3://…`.
#[derive(Debug, Clone)]
pub struct BlobHandle {
    pub uri: String,
    pub size_bytes: u64,
    pub sha256: String,
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
}

/// Relational metadata operations. Split out so handlers can be unit-tested
/// against an in-memory SQLite or a future mock without mocking HTTP.
#[async_trait]
pub trait MetadataStore: Send + Sync + 'static {
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

    // ----- files / entries (parse-on-ingest sister PR) -----

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
}
