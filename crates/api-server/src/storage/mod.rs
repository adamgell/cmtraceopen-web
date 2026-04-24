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
// `sqlite` / `postgres` modules gate on their respective cargo features —
// the sqlx driver for the feature-off case isn't compiled in (see the
// `sqlite` / `postgres` sections in `Cargo.toml`), so the module source
// won't even parse without the matching flag. Mirrors the existing
// `postgres` gate.
#[cfg(feature = "sqlite")]
pub mod meta_sqlite;
#[cfg(feature = "sqlite")]
pub mod audit_sqlite;
#[cfg(feature = "postgres")]
pub mod meta_postgres;

pub use blob_fs::LocalFsBlobStore;
pub use blob_object_store::ObjectStoreBlobStore;
#[cfg(feature = "sqlite")]
pub use meta_sqlite::SqliteMetadataStore;
#[cfg(feature = "sqlite")]
pub use audit_sqlite::AuditSqliteStore;
#[cfg(feature = "postgres")]
pub use meta_postgres::PgMetadataStore;

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

/// Bundle of handles returned by [`build_metadata_store`] — main.rs needs
/// all three today, and the concrete backend is the only place that knows
/// how to wire them up from a single pool.
///
/// Why a bundle rather than `Arc<dyn MetadataStore>` alone? Three reasons:
///
///   * [`ConfigStore`] is a **separate** trait, not a supertrait of
///     [`MetadataStore`]. main.rs holds `Arc<dyn ConfigStore>` on the
///     AppState, so the factory has to hand one back. The concrete backends
///     implement both traits on the same struct, so the bundle fans a
///     single `Arc<Concrete>` into two trait-object Arcs without opening
///     two pools.
///   * The audit store currently lives on an inherent method on the
///     concrete type (see [`SqliteMetadataStore::audit_store`]) so the
///     default [`MetadataStore::audit_store`] impl can't be relied on for
///     all backends (Postgres panics). Returning a prebuilt
///     `Arc<dyn AuditStore>` lets the factory pick the right audit strategy
///     per backend — real store for SQLite, [`NoopAuditStore`] for Postgres
///     (the PG audit-log table isn't migrated yet; see issue #110 and
///     `meta_postgres.rs`).
///   * Keeps the caller (main.rs) ignorant of the concrete type, which is
///     the whole point of the factory.
pub struct MetadataStoreBundle {
    pub meta: Arc<dyn MetadataStore>,
    pub configs: Arc<dyn ConfigStore>,
    pub audit: Arc<dyn AuditStore>,
}

/// Build the right [`MetadataStore`] implementation based on the
/// `CMTRACE_DATABASE_URL` scheme in `config.database_url`:
///
/// - `postgres://…` or `postgresql://…` → [`PgMetadataStore`] (requires the
///   `postgres` cargo feature).
/// - `sqlite://…` or a bare path → [`SqliteMetadataStore`] (requires the
///   `sqlite` cargo feature).
///
/// Returns a [`MetadataStoreBundle`] so main.rs gets the metadata, config,
/// and audit handles it needs from a single call without naming a concrete
/// store type.
///
/// This is the *only* place the codebase chooses a metadata backend. Adding a
/// new backend later means: new feature flag in `Cargo.toml`, new arm here,
/// new factory module — handlers and tests don't move.
pub async fn build_metadata_store(
    config: &crate::config::Config,
) -> Result<MetadataStoreBundle, BuildMetadataStoreError> {
    let url = &config.database_url;

    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        build_pg_metadata_store(url).await
    } else {
        build_sqlite_metadata_store(url).await
    }
}

#[cfg(feature = "postgres")]
async fn build_pg_metadata_store(
    url: &str,
) -> Result<MetadataStoreBundle, BuildMetadataStoreError> {
    // One concrete `Arc<PgMetadataStore>` fans out into the two trait-object
    // Arcs main.rs expects. The audit handle is [`NoopAuditStore`]
    // because the PG audit_log migration doesn't exist yet (see
    // `meta_postgres.rs` — PgMetadataStore::audit_store panics on purpose).
    // main.rs emits a startup warn! describing this fallback so operators
    // know audit writes are being dropped on the PG backend.
    //
    // The config-override handle is [`NoopConfigStore`] for the same
    // reason: `migrations-pg/` doesn't carry a device_config_overrides
    // migration yet, and there's no PgConfigStore impl. This matches the
    // audit strategy — surface the gap loudly at startup rather than
    // silently panic mid-request. Follow-up work (issue #110 tracks both
    // tables) is called out in the factory doc and main.rs warn!.
    let store = PgMetadataStore::connect(url).await?;
    let store: Arc<PgMetadataStore> = Arc::new(store);
    Ok(MetadataStoreBundle {
        meta: store.clone() as Arc<dyn MetadataStore>,
        configs: Arc::new(NoopConfigStore) as Arc<dyn ConfigStore>,
        audit: Arc::new(NoopAuditStore) as Arc<dyn AuditStore>,
    })
}

#[cfg(not(feature = "postgres"))]
async fn build_pg_metadata_store(
    _url: &str,
) -> Result<MetadataStoreBundle, BuildMetadataStoreError> {
    Err(BuildMetadataStoreError::PostgresFeatureMissing)
}

#[cfg(feature = "sqlite")]
async fn build_sqlite_metadata_store(
    url: &str,
) -> Result<MetadataStoreBundle, BuildMetadataStoreError> {
    // Strip the `sqlite://` scheme prefix if present; SqliteMetadataStore
    // expects either a bare path or `:memory:`.
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);
    let store = SqliteMetadataStore::connect(path).await?;
    let store: Arc<SqliteMetadataStore> = Arc::new(store);
    // The audit store shares the SQLite pool — the inherent
    // `SqliteMetadataStore::audit_store()` returns the concrete
    // `AuditSqliteStore` (cheap Arc bump on the pool). Wrap in
    // `Arc<dyn AuditStore>` so the bundle caller never names the concrete
    // type, mirroring the old inline construction in main.rs.
    let audit: Arc<dyn AuditStore> = Arc::new(store.audit_store());
    Ok(MetadataStoreBundle {
        meta: store.clone() as Arc<dyn MetadataStore>,
        configs: store as Arc<dyn ConfigStore>,
        audit,
    })
}

#[cfg(not(feature = "sqlite"))]
async fn build_sqlite_metadata_store(
    _url: &str,
) -> Result<MetadataStoreBundle, BuildMetadataStoreError> {
    Err(BuildMetadataStoreError::SqliteFeatureMissing)
}

/// Errors surfaced by [`build_metadata_store`].
#[derive(Debug, Error)]
pub enum BuildMetadataStoreError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error(
        "database URL uses a postgres:// scheme but the api-server was built \
         without the `postgres` cargo feature. Rebuild with \
         `--features postgres`."
    )]
    PostgresFeatureMissing,

    #[error(
        "database URL uses a sqlite:// scheme but the api-server was built \
         without the `sqlite` cargo feature. Rebuild with \
         `--features sqlite` (the default)."
    )]
    SqliteFeatureMissing,
}


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
            database_url: "sqlite::memory:".to_string(),
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
            rate_limit: Default::default(),
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

    /// Exercise the metadata-store factory on the sqlite path: given a
    /// `sqlite::memory:` URL, `build_metadata_store` should return a
    /// bundle with all three trait-object Arcs populated and each one
    /// functional (i.e. the returned store implements its trait contract,
    /// not a stub).
    ///
    /// No equivalent test exists for the Postgres path — exercising it
    /// requires a live PG instance that this crate's unit-test harness
    /// doesn't spin up. A `#[ignore]`d integration test is the right home
    /// for that and is called out as a follow-up in the commit body.
    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn factory_returns_sqlite_bundle_for_sqlite_url() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = base_config(tmp.path().to_path_buf());
        cfg.database_url = "sqlite::memory:".to_string();

        let bundle = build_metadata_store(&cfg)
            .await
            .expect("sqlite backend should build for in-memory URL");

        // `Arc<dyn MetadataStore>` — touch the surface to prove it's a
        // real store, not the default-impl NoopAuditStore stub.
        let stats = bundle.meta.pool_stats();
        assert!(
            stats.max_size > 0,
            "SqliteMetadataStore::pool_stats should report a non-zero max_size",
        );

        // `Arc<dyn ConfigStore>` — read on an empty DB returns None.
        let cfg_row = bundle
            .configs
            .get_default_config()
            .await
            .expect("get_default_config should not error on empty DB");
        assert!(
            cfg_row.is_none(),
            "default config should be absent on a fresh in-memory DB",
        );

        // `Arc<dyn AuditStore>` — list returns empty on a fresh DB.
        let rows = bundle
            .audit
            .list_audit_rows(&AuditFilters::default(), 10)
            .await
            .expect("list_audit_rows should not error on empty DB");
        assert!(rows.is_empty(), "audit log should be empty on fresh DB");
    }

    /// Also exercise the bare-`:memory:` URL form (no `sqlite:` prefix)
    /// to lock the factory's `unwrap_or(url)` fallback behavior in place.
    /// The Docker/compose path uses a `sqlite://...` prefix, but the
    /// inherent `SqliteMetadataStore::connect` accepts bare paths too and
    /// the factory preserves that.
    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn factory_accepts_bare_memory_url() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = base_config(tmp.path().to_path_buf());
        cfg.database_url = ":memory:".to_string();

        let bundle = build_metadata_store(&cfg)
            .await
            .expect("sqlite backend should build for bare :memory: URL");
        // Smoke-test the bundle: every Arc populated.
        let _ = bundle.meta.pool_stats();
        let _ = bundle
            .audit
            .list_audit_rows(&AuditFilters::default(), 1)
            .await
            .unwrap();
    }

    /// When the binary is built without the `postgres` feature, a
    /// `postgres://` URL must surface
    /// [`BuildMetadataStoreError::PostgresFeatureMissing`] rather than
    /// silently falling back to sqlite.
    #[cfg(not(feature = "postgres"))]
    #[tokio::test]
    async fn factory_rejects_postgres_when_feature_disabled() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = base_config(tmp.path().to_path_buf());
        cfg.database_url = "postgres://user:pass@localhost/db".to_string();

        // `MetadataStoreBundle` doesn't implement Debug, so expect_err
        // doesn't compile here — match the result manually instead.
        match build_metadata_store(&cfg).await {
            Ok(_) => panic!("postgres backend should fail when feature disabled"),
            Err(BuildMetadataStoreError::PostgresFeatureMissing) => {}
            Err(other) => panic!("expected PostgresFeatureMissing, got {other:?}"),
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

use common_wire::AgentConfigOverride;

/// Config-override storage operations used by the server-side config push
/// feature (Wave 4).  Split out as its own trait so it can be layered on
/// top of any [`MetadataStore`] implementation without widening the base
/// interface.
#[async_trait]
pub trait ConfigStore: Send + Sync + 'static {
    /// Return the per-device config override for `device_id`, or `None` if no
    /// override has been set for that device.
    async fn get_device_config(
        &self,
        device_id: &str,
    ) -> Result<Option<AgentConfigOverride>, StorageError>;

    /// Upsert the per-device config override for `device_id`.
    async fn set_device_config(
        &self,
        device_id: &str,
        config: &AgentConfigOverride,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError>;

    /// Remove any per-device config override for `device_id`.  Returns `Ok`
    /// even if no row existed.
    async fn delete_device_config(&self, device_id: &str) -> Result<(), StorageError>;

    /// Return the tenant-wide default config override, or `None` if not set.
    async fn get_default_config(&self) -> Result<Option<AgentConfigOverride>, StorageError>;

    /// Upsert the tenant-wide default config override.
    async fn set_default_config(
        &self,
        config: &AgentConfigOverride,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError>;
}

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

    /// Build an [`AuditStore`] that shares this metadata store's connection
    /// pool. Called once at startup in `main.rs` so the audit log writes go
    /// through the same pool as metadata writes.
    ///
    /// The default impl returns a [`NoopAuditStore`] — backends that don't
    /// have a real audit-log table (mocks, future in-memory stores) don't
    /// need to override this. Production backends MUST override.
    fn audit_store(&self) -> Arc<dyn AuditStore> {
        Arc::new(NoopAuditStore)
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

    /// Aggregate count of sessions grouped by `parse_state`, ordered by
    /// count DESC. Used by the dev status page (`GET /`) to surface a
    /// "parse-state distribution" card spanning **all** sessions (not just
    /// the recent-N window). Returns `(state, count)` tuples — the state
    /// column is `TEXT` so callers must handle arbitrary strings, including
    /// future states the renderer hasn't mapped yet.
    ///
    /// Default impl returns an empty vec so backends without a dedicated
    /// grouped-count query (mocks, future in-memory stores, the current
    /// Postgres impl which has no migration for this yet) don't have to
    /// scramble. The status page renders the empty case as a muted
    /// placeholder, mirroring how `recent_sessions` is treated.
    async fn count_sessions_by_state(&self) -> Result<Vec<(String, u64)>, StorageError> {
        Ok(Vec::new())
    }

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

/// Optional filters + keyset cursor for [`AuditStore::list_audit_rows`].
///
/// Pagination uses a **keyset cursor** — the caller passes back the
/// `(ts_utc, id)` of the last row from the previous page as
/// [`Self::cursor_before`], and the next page returns rows with
/// `(ts_utc, id) < (cursor_ts, cursor_id)` in lexicographic order. This
/// matches the convention used by `routes/sessions.rs` and avoids the
/// "two rows in the same `ts_utc` second tie and pagination drops or
/// duplicates them" bug that an offset/limit or `after_ts`-only scheme
/// suffers under high write volume.
///
/// All fields default to `None`, meaning "no filter applied".
#[derive(Debug, Clone, Default)]
pub struct AuditFilters {
    /// Keyset cursor: include only rows where
    /// `(ts_utc, id) < (cursor_ts, cursor_id)` (strict lexicographic
    /// comparison). The composite-comparison form means rows tied on
    /// `ts_utc` to the same second are still strictly ordered by `id` (UUID
    /// v7 — time-sortable insertion order), so paging never drops or
    /// duplicates rows.
    pub cursor_before: Option<(DateTime<Utc>, Uuid)>,
    /// Include only rows whose `principal_id` equals this value.
    pub principal: Option<String>,
    /// Include only rows whose `action` equals this value.
    pub action: Option<String>,
}

/// Insert-only audit log of admin/operator actions.
///
/// Implementations MUST never update or delete rows. This is a trait-level
/// contract enforced by the SQLite/Postgres impls' API surface — they do
/// not expose UPDATE or DELETE methods on this trait. **Note**: this is an
/// application-layer guarantee only; the underlying database has no
/// constraint that prevents a DBA / compromised process from mutating
/// rows. Cryptographic tamper evidence (hash chain + verifier endpoint)
/// is tracked as a follow-up — see issue #110 and
/// `docs/adr/0001-postgres-storage-types.md`.
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

/// No-op [`ConfigStore`] used as a fallback when a backend doesn't yet have
/// a real implementation.
///
/// Reads return `None` (i.e. "no override configured"); writes and deletes
/// silently succeed. This parallels [`NoopAuditStore`] and exists for the
/// same reason: the Postgres backend has neither the
/// `device_config_overrides` nor `default_config_override` tables migrated
/// (`migrations-pg/` is missing an equivalent of
/// `migrations/0004_device_config.sql`), and there is no `PgConfigStore`
/// impl. The factory routes PG to this fallback so `build_metadata_store`
/// can return a uniform bundle; `main.rs` warns loudly at startup when the
/// selected backend is Postgres so the operator knows config-push writes
/// are being silently dropped until a real impl lands. See issue #110.
pub struct NoopConfigStore;

#[async_trait]
impl ConfigStore for NoopConfigStore {
    async fn get_device_config(
        &self,
        _device_id: &str,
    ) -> Result<Option<AgentConfigOverride>, StorageError> {
        Ok(None)
    }

    async fn set_device_config(
        &self,
        _device_id: &str,
        _config: &AgentConfigOverride,
        _now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    async fn delete_device_config(&self, _device_id: &str) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_default_config(&self) -> Result<Option<AgentConfigOverride>, StorageError> {
        Ok(None)
    }

    async fn set_default_config(
        &self,
        _config: &AgentConfigOverride,
        _now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        Ok(())
    }
}
