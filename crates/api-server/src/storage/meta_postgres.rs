//! PostgreSQL-backed [`MetadataStore`].
//!
//! Mirrors `meta_sqlite.rs` but targets `PgPool`. Key differences from the
//! SQLite implementation:
//!
//! * **Placeholders**: Postgres uses positional `$1`, `$2`, … instead of `?`.
//! * **Timestamps**: stored and queried as `TEXT` (ISO-8601 / RFC-3339) to
//!   keep the same chrono serialisation logic as the SQLite backend. The
//!   column type is `TEXT` in `migrations-pg/`; Postgres is happy to index
//!   and compare TEXT lexicographically, which works because RFC-3339 strings
//!   sort identically to their epoch values when zero-padded (our
//!   `DateTime::to_rfc3339()` output always is).
//! * **NULLS LAST**: Postgres natively supports the SQL:2003 `NULLS LAST`
//!   clause in `ORDER BY`, so the `(ts_ms IS NULL) ASC` trick used in the
//!   SQLite backend is replaced with the more readable standard form.
//! * **Auto-increment PK**: `entries.entry_id` is `BIGSERIAL` in Postgres
//!   (vs `INTEGER PRIMARY KEY AUTOINCREMENT` in SQLite). Both surface as `i64`
//!   in Rust.
//! * **Unique-constraint error detection**: Postgres surfaces unique violations
//!   as error code `23505`; the insert_session conflict handler checks for
//!   that code instead of the SQLite "unique constraint failed" substring.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use uuid::Uuid;

use super::{
    AuditStore, DeviceRow, EntryFilters, EntryRow, FileRow, MetadataStore, NewEntry, NewFile,
    NewUpload, PoolStats, SessionRow, StorageError, UploadRow,
};

/// Bake the Postgres migration directory into the binary. Path is relative to
/// this crate's `Cargo.toml` (the manifest dir).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations-pg");

/// Max connections the pool is permitted to grow to.
const POOL_MAX_CONNECTIONS: u32 = 16;

#[derive(Clone)]
pub struct PgMetadataStore {
    pool: PgPool,
}

impl PgMetadataStore {
    /// Connect to a Postgres database at `url` and run pending migrations.
    ///
    /// `url` must be a valid `postgres://` or `postgresql://` connection string.
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let opts = PgConnectOptions::from_str(url)?;

        let pool = PgPoolOptions::new()
            .max_connections(POOL_MAX_CONNECTIONS)
            .connect_with(opts)
            .await?;

        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Access the underlying pool.
    ///
    /// Public because integration tests in the `tests/` directory are a
    /// separate compilation unit and need to seed/assert against tables
    /// directly. Not intended for production use outside of tests.
    #[doc(hidden)]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(s).map_err(|e| {
        StorageError::Sqlx(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid uuid in db: {e}"),
        ))))
    })
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            StorageError::Sqlx(sqlx::Error::Decode(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid timestamp in db: {e}"),
            ))))
        })
}

fn parse_ts_opt(s: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    s.map(|v| parse_ts(&v)).transpose()
}

#[async_trait]
impl MetadataStore for PgMetadataStore {
    fn pool_stats(&self) -> PoolStats {
        PoolStats {
            size: self.pool.size(),
            idle: u32::try_from(self.pool.num_idle()).unwrap_or(u32::MAX),
            max_size: POOL_MAX_CONNECTIONS,
        }
    }

    fn audit_store(&self) -> std::sync::Arc<dyn AuditStore> {
        // The Postgres audit-log table isn't migrated yet — see issue #110
        // and `migrations-pg/` (currently lacks an `audit_log` migration
        // matching `migrations/0003_audit_log.sql`). A real PgAuditStore
        // lands together with that migration; until then, panic loudly so
        // an operator who selects the postgres backend without the audit
        // table provisioned gets a clear "not yet implemented" rather than
        // silent data loss from a NoopAuditStore.
        unimplemented!(
            "PgMetadataStore::audit_store: Postgres audit-log support not yet \
             implemented (issue #110). Use the sqlite backend or wait for \
             the audit_log migration in `migrations-pg/`."
        )
    }

    async fn upsert_device(
        &self,
        device_id: &str,
        hostname: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let now_s = now.to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO devices (device_id, first_seen_utc, last_seen_utc, hostname)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT(device_id) DO UPDATE SET
                last_seen_utc = EXCLUDED.last_seen_utc,
                hostname = COALESCE(EXCLUDED.hostname, devices.hostname)
            "#,
        )
        .bind(device_id)
        .bind(&now_s)
        .bind(&now_s)
        .bind(hostname)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_devices(
        &self,
        limit: u32,
        after_device_id: Option<&str>,
    ) -> Result<Vec<DeviceRow>, StorageError> {
        let limit_i = limit as i64;
        let rows = if let Some(after) = after_device_id {
            sqlx::query(
                r#"
                SELECT d.device_id, d.first_seen_utc, d.last_seen_utc, d.hostname,
                       (SELECT COUNT(*) FROM sessions s WHERE s.device_id = d.device_id) AS session_count
                FROM devices d
                WHERE d.device_id > $1
                ORDER BY d.device_id ASC
                LIMIT $2
                "#,
            )
            .bind(after)
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT d.device_id, d.first_seen_utc, d.last_seen_utc, d.hostname,
                       (SELECT COUNT(*) FROM sessions s WHERE s.device_id = d.device_id) AS session_count
                FROM devices d
                ORDER BY d.device_id ASC
                LIMIT $1
                "#,
            )
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(DeviceRow {
                device_id: r.get::<String, _>("device_id"),
                first_seen_utc: parse_ts(&r.get::<String, _>("first_seen_utc"))?,
                last_seen_utc: parse_ts(&r.get::<String, _>("last_seen_utc"))?,
                hostname: r.get::<Option<String>, _>("hostname"),
                session_count: r.get::<i64, _>("session_count"),
            });
        }
        Ok(out)
    }

    async fn insert_upload(
        &self,
        new: NewUpload,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO uploads
              (upload_id, bundle_id, device_id, size_bytes, expected_sha256,
               content_kind, offset_bytes, staged_path, created_utc, finalized)
            VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $8, 0)
            "#,
        )
        .bind(new.upload_id.to_string())
        .bind(new.bundle_id.to_string())
        .bind(&new.device_id)
        .bind(new.size_bytes as i64)
        .bind(&new.expected_sha256)
        .bind(&new.content_kind)
        .bind(&new.staged_path)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_upload(&self, upload_id: Uuid) -> Result<UploadRow, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT upload_id, bundle_id, device_id, size_bytes, expected_sha256,
                   content_kind, offset_bytes, staged_path, created_utc, finalized
            FROM uploads WHERE upload_id = $1
            "#,
        )
        .bind(upload_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::UploadNotFound(upload_id))?;

        Ok(UploadRow {
            upload_id: parse_uuid(&row.get::<String, _>("upload_id"))?,
            bundle_id: parse_uuid(&row.get::<String, _>("bundle_id"))?,
            device_id: row.get::<String, _>("device_id"),
            size_bytes: row.get::<i64, _>("size_bytes") as u64,
            expected_sha256: row.get::<String, _>("expected_sha256"),
            content_kind: row.get::<String, _>("content_kind"),
            offset_bytes: row.get::<i64, _>("offset_bytes") as u64,
            staged_path: row.get::<String, _>("staged_path"),
            created_utc: parse_ts(&row.get::<String, _>("created_utc"))?,
            finalized: row.get::<i32, _>("finalized") != 0,
        })
    }

    async fn set_upload_offset(
        &self,
        upload_id: Uuid,
        new_offset: u64,
    ) -> Result<(), StorageError> {
        let res = sqlx::query("UPDATE uploads SET offset_bytes = $1 WHERE upload_id = $2")
            .bind(new_offset as i64)
            .bind(upload_id.to_string())
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StorageError::UploadNotFound(upload_id));
        }
        Ok(())
    }

    async fn compare_and_set_upload_offset(
        &self,
        upload_id: Uuid,
        expected_offset: u64,
        new_offset: u64,
    ) -> Result<bool, StorageError> {
        let res = sqlx::query(
            "UPDATE uploads SET offset_bytes = $1 \
             WHERE upload_id = $2 AND offset_bytes = $3",
        )
        .bind(new_offset as i64)
        .bind(upload_id.to_string())
        .bind(expected_offset as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 1 {
            return Ok(true);
        }
        let exists = sqlx::query("SELECT 1 FROM uploads WHERE upload_id = $1")
            .bind(upload_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .is_some();
        if exists {
            Ok(false)
        } else {
            Err(StorageError::UploadNotFound(upload_id))
        }
    }

    async fn mark_upload_finalized(&self, upload_id: Uuid) -> Result<(), StorageError> {
        let res = sqlx::query("UPDATE uploads SET finalized = 1 WHERE upload_id = $1")
            .bind(upload_id.to_string())
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StorageError::UploadNotFound(upload_id));
        }
        Ok(())
    }

    async fn find_resumable_upload(
        &self,
        device_id: &str,
        bundle_id: Uuid,
    ) -> Result<Option<UploadRow>, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT upload_id, bundle_id, device_id, size_bytes, expected_sha256,
                   content_kind, offset_bytes, staged_path, created_utc, finalized
            FROM uploads
            WHERE device_id = $1 AND bundle_id = $2 AND finalized = 0
            ORDER BY created_utc DESC
            LIMIT 1
            "#,
        )
        .bind(device_id)
        .bind(bundle_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok::<_, StorageError>(UploadRow {
                upload_id: parse_uuid(&row.get::<String, _>("upload_id"))?,
                bundle_id: parse_uuid(&row.get::<String, _>("bundle_id"))?,
                device_id: row.get::<String, _>("device_id"),
                size_bytes: row.get::<i64, _>("size_bytes") as u64,
                expected_sha256: row.get::<String, _>("expected_sha256"),
                content_kind: row.get::<String, _>("content_kind"),
                offset_bytes: row.get::<i64, _>("offset_bytes") as u64,
                staged_path: row.get::<String, _>("staged_path"),
                created_utc: parse_ts(&row.get::<String, _>("created_utc"))?,
                finalized: row.get::<i32, _>("finalized") != 0,
            })
        })
        .transpose()
    }

    async fn insert_session(&self, row: SessionRow) -> Result<(), StorageError> {
        let res = sqlx::query(
            r#"
            INSERT INTO sessions
              (session_id, device_id, bundle_id, blob_uri, content_kind,
               size_bytes, sha256, collected_utc, ingested_utc, parse_state)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(row.session_id.to_string())
        .bind(&row.device_id)
        .bind(row.bundle_id.to_string())
        .bind(&row.blob_uri)
        .bind(&row.content_kind)
        .bind(row.size_bytes as i64)
        .bind(&row.sha256)
        .bind(row.collected_utc.map(|t| t.to_rfc3339()))
        .bind(row.ingested_utc.to_rfc3339())
        .bind(&row.parse_state)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(dbe)) if dbe.code().as_deref() == Some("23505") => {
                // Postgres unique-constraint violation error code 23505.
                Err(StorageError::SessionConflict {
                    device_id: row.device_id.clone(),
                    bundle_id: row.bundle_id,
                })
            }
            Err(e) => Err(StorageError::Sqlx(e)),
        }
    }

    async fn find_session_by_bundle(
        &self,
        device_id: &str,
        bundle_id: Uuid,
    ) -> Result<Option<SessionRow>, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                   size_bytes, sha256, collected_utc, ingested_utc, parse_state
            FROM sessions WHERE device_id = $1 AND bundle_id = $2
            "#,
        )
        .bind(device_id)
        .bind(bundle_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok::<_, StorageError>(SessionRow {
                session_id: parse_uuid(&row.get::<String, _>("session_id"))?,
                device_id: row.get::<String, _>("device_id"),
                bundle_id: parse_uuid(&row.get::<String, _>("bundle_id"))?,
                blob_uri: row.get::<String, _>("blob_uri"),
                content_kind: row.get::<String, _>("content_kind"),
                size_bytes: row.get::<i64, _>("size_bytes") as u64,
                sha256: row.get::<String, _>("sha256"),
                collected_utc: parse_ts_opt(row.get::<Option<String>, _>("collected_utc"))?,
                ingested_utc: parse_ts(&row.get::<String, _>("ingested_utc"))?,
                parse_state: row.get::<String, _>("parse_state"),
            })
        })
        .transpose()
    }

    async fn get_session(&self, session_id: Uuid) -> Result<Option<SessionRow>, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                   size_bytes, sha256, collected_utc, ingested_utc, parse_state
            FROM sessions WHERE session_id = $1
            "#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok::<_, StorageError>(SessionRow {
                session_id: parse_uuid(&row.get::<String, _>("session_id"))?,
                device_id: row.get::<String, _>("device_id"),
                bundle_id: parse_uuid(&row.get::<String, _>("bundle_id"))?,
                blob_uri: row.get::<String, _>("blob_uri"),
                content_kind: row.get::<String, _>("content_kind"),
                size_bytes: row.get::<i64, _>("size_bytes") as u64,
                sha256: row.get::<String, _>("sha256"),
                collected_utc: parse_ts_opt(row.get::<Option<String>, _>("collected_utc"))?,
                ingested_utc: parse_ts(&row.get::<String, _>("ingested_utc"))?,
                parse_state: row.get::<String, _>("parse_state"),
            })
        })
        .transpose()
    }

    async fn list_sessions_for_device(
        &self,
        device_id: &str,
        limit: u32,
        before: Option<(DateTime<Utc>, Uuid)>,
    ) -> Result<Vec<SessionRow>, StorageError> {
        let limit_i = limit as i64;
        let rows = if let Some((ts, sid)) = before {
            sqlx::query(
                r#"
                SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                       size_bytes, sha256, collected_utc, ingested_utc, parse_state
                FROM sessions
                WHERE device_id = $1
                  AND (ingested_utc < $2
                       OR (ingested_utc = $3 AND session_id < $4))
                ORDER BY ingested_utc DESC, session_id DESC
                LIMIT $5
                "#,
            )
            .bind(device_id)
            .bind(ts.to_rfc3339())
            .bind(ts.to_rfc3339())
            .bind(sid.to_string())
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                       size_bytes, sha256, collected_utc, ingested_utc, parse_state
                FROM sessions
                WHERE device_id = $1
                ORDER BY ingested_utc DESC, session_id DESC
                LIMIT $2
                "#,
            )
            .bind(device_id)
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(SessionRow {
                session_id: parse_uuid(&r.get::<String, _>("session_id"))?,
                device_id: r.get::<String, _>("device_id"),
                bundle_id: parse_uuid(&r.get::<String, _>("bundle_id"))?,
                blob_uri: r.get::<String, _>("blob_uri"),
                content_kind: r.get::<String, _>("content_kind"),
                size_bytes: r.get::<i64, _>("size_bytes") as u64,
                sha256: r.get::<String, _>("sha256"),
                collected_utc: parse_ts_opt(r.get::<Option<String>, _>("collected_utc"))?,
                ingested_utc: parse_ts(&r.get::<String, _>("ingested_utc"))?,
                parse_state: r.get::<String, _>("parse_state"),
            });
        }
        Ok(out)
    }

    async fn recent_sessions(&self, limit: u32) -> Result<Vec<SessionRow>, StorageError> {
        let limit_i = limit as i64;
        let rows = sqlx::query(
            r#"
            SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                   size_bytes, sha256, collected_utc, ingested_utc, parse_state
            FROM sessions
            ORDER BY ingested_utc DESC, session_id DESC
            LIMIT $1
            "#,
        )
        .bind(limit_i)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(SessionRow {
                session_id: parse_uuid(&r.get::<String, _>("session_id"))?,
                device_id: r.get::<String, _>("device_id"),
                bundle_id: parse_uuid(&r.get::<String, _>("bundle_id"))?,
                blob_uri: r.get::<String, _>("blob_uri"),
                content_kind: r.get::<String, _>("content_kind"),
                size_bytes: r.get::<i64, _>("size_bytes") as u64,
                sha256: r.get::<String, _>("sha256"),
                collected_utc: parse_ts_opt(r.get::<Option<String>, _>("collected_utc"))?,
                ingested_utc: parse_ts(&r.get::<String, _>("ingested_utc"))?,
                parse_state: r.get::<String, _>("parse_state"),
            });
        }
        Ok(out)
    }

    async fn update_session_parse_state(
        &self,
        session_id: Uuid,
        state: &str,
    ) -> Result<(), StorageError> {
        let res = sqlx::query("UPDATE sessions SET parse_state = $1 WHERE session_id = $2")
            .bind(state)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StorageError::Sqlx(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    async fn insert_file(&self, new: NewFile) -> Result<Uuid, StorageError> {
        sqlx::query(
            r#"
            INSERT INTO files
              (file_id, session_id, relative_path, size_bytes,
               format_detected, parser_kind, entry_count, parse_error_count)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(new.file_id.to_string())
        .bind(new.session_id.to_string())
        .bind(&new.relative_path)
        .bind(new.size_bytes as i64)
        .bind(new.format_detected.as_deref())
        .bind(new.parser_kind.as_deref())
        .bind(new.entry_count as i64)
        .bind(new.parse_error_count as i64)
        .execute(&self.pool)
        .await?;
        Ok(new.file_id)
    }

    async fn insert_entries_batch(&self, entries: Vec<NewEntry>) -> Result<(), StorageError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for e in entries {
            sqlx::query(
                r#"
                INSERT INTO entries
                  (session_id, file_id, line_number, ts_ms,
                   severity, component, thread, message, extras_json)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                "#,
            )
            .bind(e.session_id.to_string())
            .bind(e.file_id.to_string())
            .bind(e.line_number as i64)
            .bind(e.ts_ms)
            .bind(e.severity as i64)
            .bind(e.component.as_deref())
            .bind(e.thread.as_deref())
            .bind(&e.message)
            .bind(e.extras_json.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_files_for_session(
        &self,
        session_id: Uuid,
        limit: u32,
        after_file_id: Option<&str>,
    ) -> Result<Vec<FileRow>, StorageError> {
        let limit_i = limit as i64;
        let rows = if let Some(after) = after_file_id {
            sqlx::query(
                r#"
                SELECT file_id, session_id, relative_path, size_bytes,
                       format_detected, parser_kind, entry_count, parse_error_count
                FROM files
                WHERE session_id = $1 AND file_id > $2
                ORDER BY file_id ASC
                LIMIT $3
                "#,
            )
            .bind(session_id.to_string())
            .bind(after)
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT file_id, session_id, relative_path, size_bytes,
                       format_detected, parser_kind, entry_count, parse_error_count
                FROM files
                WHERE session_id = $1
                ORDER BY file_id ASC
                LIMIT $2
                "#,
            )
            .bind(session_id.to_string())
            .bind(limit_i)
            .fetch_all(&self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(FileRow {
                file_id: r.get::<String, _>("file_id"),
                session_id: r.get::<String, _>("session_id"),
                relative_path: r.get::<String, _>("relative_path"),
                size_bytes: r.get::<i64, _>("size_bytes") as u64,
                format_detected: r.get::<Option<String>, _>("format_detected"),
                parser_kind: r.get::<Option<String>, _>("parser_kind"),
                entry_count: r.get::<i64, _>("entry_count") as u64,
                parse_error_count: r.get::<i64, _>("parse_error_count") as u64,
            });
        }
        Ok(out)
    }

    async fn query_entries(
        &self,
        session_id: Uuid,
        filters: &EntryFilters,
        limit: u32,
    ) -> Result<Vec<EntryRow>, StorageError> {
        // Dynamic query assembly with positional Postgres parameters ($N).
        // We track the next placeholder index in `param_idx` and push each
        // bind value into a `Vec<Param>` in the same order the `$N` tokens
        // appear in `sql`. Every value is bound, never interpolated — no user
        // input reaches the SQL text.
        //
        // Ordering: `ts_ms ASC NULLS LAST, entry_id ASC`.
        // Postgres natively supports NULLS LAST; no workaround needed.
        // Keyset cursor logic:
        //   a) cursor.ts_ms is Some: next page continues past (ts, entry_id)
        //      in the non-null tier, OR anywhere in the null tier.
        //   b) cursor.ts_ms is None: walking the null tier; continue with
        //      entry_id > cursor.entry_id inside IS NULL rows.
        let mut sql = String::from(
            "SELECT entry_id, file_id, line_number, ts_ms, severity, \
             component, thread, message, extras_json \
             FROM entries WHERE session_id = $1",
        );

        enum Param<'a> {
            Str(&'a str),
            OwnedStr(String),
            I64(i64),
        }
        let mut params: Vec<Param> = Vec::with_capacity(8);
        params.push(Param::OwnedStr(session_id.to_string()));
        let mut param_idx: usize = 2; // next placeholder number

        let mut next = || {
            let n = param_idx;
            param_idx += 1;
            format!("${n}")
        };

        if let Some(ref fid) = filters.file_id {
            sql.push_str(&format!(" AND file_id = {}", next()));
            params.push(Param::Str(fid));
        }
        if let Some(sev) = filters.min_severity {
            sql.push_str(&format!(" AND severity >= {}", next()));
            params.push(Param::I64(sev));
        }
        if let Some(after) = filters.after_ts_ms {
            sql.push_str(&format!(" AND ts_ms IS NOT NULL AND ts_ms >= {}", next()));
            params.push(Param::I64(after));
        }
        if let Some(before) = filters.before_ts_ms {
            sql.push_str(&format!(" AND ts_ms IS NOT NULL AND ts_ms < {}", next()));
            params.push(Param::I64(before));
        }
        if let Some(ref q) = filters.q_like {
            sql.push_str(&format!(" AND message LIKE {}", next()));
            params.push(Param::Str(q));
        }
        if let Some(ref c) = filters.cursor {
            match c.ts_ms {
                Some(ts) => {
                    let p_ts1 = next();
                    let p_ts2 = next();
                    let p_eid = next();
                    sql.push_str(&format!(
                        " AND ( \
                           (ts_ms IS NOT NULL AND (ts_ms > {p_ts1} OR (ts_ms = {p_ts2} AND entry_id > {p_eid}))) \
                           OR ts_ms IS NULL \
                         )",
                    ));
                    params.push(Param::I64(ts));
                    params.push(Param::I64(ts));
                    params.push(Param::I64(c.entry_id));
                }
                None => {
                    let p_eid = next();
                    sql.push_str(&format!(" AND ts_ms IS NULL AND entry_id > {p_eid}"));
                    params.push(Param::I64(c.entry_id));
                }
            }
        }

        let p_limit = next();
        sql.push_str(&format!(" ORDER BY ts_ms ASC NULLS LAST, entry_id ASC LIMIT {p_limit}"));
        params.push(Param::I64(limit as i64));

        let mut q = sqlx::query(&sql);
        for p in &params {
            q = match p {
                Param::Str(s) => q.bind(*s),
                Param::OwnedStr(s) => q.bind(s.as_str()),
                Param::I64(n) => q.bind(*n),
            };
        }
        let rows = q.fetch_all(&self.pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(EntryRow {
                entry_id: r.get::<i64, _>("entry_id"),
                file_id: r.get::<String, _>("file_id"),
                line_number: r.get::<i32, _>("line_number") as u32,
                ts_ms: r.get::<Option<i64>, _>("ts_ms"),
                severity: r.get::<i32, _>("severity") as i64,
                component: r.get::<Option<String>, _>("component"),
                thread: r.get::<Option<String>, _>("thread"),
                message: r.get::<String, _>("message"),
                extras_json: r.get::<Option<String>, _>("extras_json"),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read `CMTRACE_DATABASE_URL` and require it to be a `postgres://` URL.
    ///
    /// These tests run only when explicitly requested via
    /// `cargo test ... -- --ignored` AND a reachable Postgres URL is in
    /// the env. The `#[ignore]` annotation on each test means they show
    /// up as `ignored` in the standard test runner output (rather than
    /// silently passing when no PG is reachable, which the previous
    /// `eprintln!` + early-return path did — that pattern hid the lack
    /// of CI coverage).
    ///
    /// To run locally with a containerised Postgres:
    /// ```bash
    /// docker run --rm -d -p 5432:5432 -e POSTGRES_PASSWORD=pw postgres:16
    /// CMTRACE_DATABASE_URL='postgres://postgres:pw@127.0.0.1:5432/postgres' \
    ///   cargo test -p api-server --features postgres meta_postgres -- --ignored
    /// ```
    ///
    /// Panics with a clear message if the env var is set but doesn't have
    /// a `postgres://` scheme — that indicates an operator typo
    /// (e.g. `sqlite://...` left over from a previous run) rather than
    /// "no PG configured", and we'd rather fail loud than silently green.
    fn pg_url_or_skip() -> String {
        let raw = match std::env::var("CMTRACE_DATABASE_URL") {
            Ok(v) if !v.trim().is_empty() => v,
            _ => panic!(
                "CMTRACE_DATABASE_URL not set; this test is gated by #[ignore] — \
                 run with `cargo test ... -- --ignored` after exporting the env var."
            ),
        };
        if !(raw.starts_with("postgres://") || raw.starts_with("postgresql://")) {
            panic!(
                "CMTRACE_DATABASE_URL is set but is not a postgres:// URL: {raw:?}. \
                 Did you mean to point at a Postgres instance? \
                 (sqlite paths cannot exercise the PG-specific code path.)"
            );
        }
        raw
    }

    #[tokio::test]
    #[ignore = "set CMTRACE_DATABASE_URL=postgres://... and run with --ignored"]
    async fn migrations_apply_and_upsert_device_works() {
        let url = pg_url_or_skip();
        let store = PgMetadataStore::connect(&url).await.unwrap();
        let now = Utc::now();
        store.upsert_device("WIN-PG-01", Some("pglab01"), now).await.unwrap();
        store.upsert_device("WIN-PG-01", None, now).await.unwrap();
        let rows = store.list_devices(10, None).await.unwrap();
        assert!(rows.iter().any(|r| r.device_id == "WIN-PG-01"));
        let r = rows.iter().find(|r| r.device_id == "WIN-PG-01").unwrap();
        assert_eq!(r.hostname.as_deref(), Some("pglab01"));
    }

    #[tokio::test]
    #[ignore = "set CMTRACE_DATABASE_URL=postgres://... and run with --ignored"]
    async fn pool_stats_reports_sane_values() {
        let url = pg_url_or_skip();
        let store = PgMetadataStore::connect(&url).await.unwrap();
        let stats = store.pool_stats();
        assert_eq!(stats.max_size, POOL_MAX_CONNECTIONS);
        assert!(stats.size <= stats.max_size, "size {} > max {}", stats.size, stats.max_size);
        assert!(stats.idle <= stats.size, "idle {} > size {}", stats.idle, stats.size);
    }
}
