//! SQLite-backed [`MetadataStore`].
//!
//! Uses sqlx's runtime-checked `query!` macros would be ideal but require a
//! prepared DB for compile-time verification, which complicates fresh
//! checkouts. MVP uses runtime `query_as`/`query` so `cargo check` works
//! without SQLX_OFFLINE data. Revisit after we stabilize the schema.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

use super::{
    DeviceRow, EntryFilters, EntryRow, FileRow, MetadataStore, NewUpload, SessionRow,
    StorageError, UploadRow,
};

/// Bake the migration directory into the binary. Path is relative to this
/// crate's `Cargo.toml` (the manifest dir).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct SqliteMetadataStore {
    pool: SqlitePool,
}

impl SqliteMetadataStore {
    /// Open (or create) a SQLite DB at `path` and run pending migrations.
    /// `path` may be `:memory:` for tests.
    pub async fn connect(path: &str) -> Result<Self, StorageError> {
        // WAL + busy_timeout on every connection:
        //
        //   * journal_mode=WAL lets multiple readers coexist with a single
        //     writer instead of the default rollback-journal's whole-DB
        //     lock, which matters as soon as the ingest + query paths share
        //     the pool under load.
        //   * busy_timeout=5s tells SQLite to sleep-and-retry instead of
        //     immediately throwing SQLITE_BUSY when a writer is mid-commit.
        //     Without it sqlx surfaces transient BUSY errors as hard 5xx
        //     even under modest contention.
        //
        // In-memory DBs ignore journal_mode (they're always MEMORY), but
        // applying the option is harmless and keeps the code path uniform
        // for tests.
        let busy = Duration::from_secs(5);
        let opts = if path == ":memory:" {
            SqliteConnectOptions::from_str("sqlite::memory:")?
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(busy)
        } else {
            // Make sure the parent directory exists so a default dev path
            // like ./data/meta.sqlite just works.
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            }
            SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
                .create_if_missing(true)
                .foreign_keys(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(busy)
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;

        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Access the underlying pool.
    ///
    /// Public because integration tests in the `tests/` directory are a
    /// separate compilation unit and need to seed tables directly (in
    /// particular, to insert `files`/`entries` rows that the
    /// parse-on-ingest sister PR will populate at runtime). Not intended
    /// for production use outside of tests.
    #[doc(hidden)]
    pub fn pool(&self) -> &SqlitePool {
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
impl MetadataStore for SqliteMetadataStore {
    async fn upsert_device(
        &self,
        device_id: &str,
        hostname: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let now_s = now.to_rfc3339();
        // SQLite upsert: insert if new, update last_seen_utc (+ hostname if we
        // got one) otherwise.
        sqlx::query(
            r#"
            INSERT INTO devices (device_id, first_seen_utc, last_seen_utc, hostname)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(device_id) DO UPDATE SET
                last_seen_utc = excluded.last_seen_utc,
                hostname = COALESCE(excluded.hostname, devices.hostname)
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
        // Keyset pagination on device_id (ascii-lex). Correlated session count
        // keeps the MVP query simple; promote to a maintained counter if this
        // becomes hot.
        let limit_i = limit as i64;
        let rows = if let Some(after) = after_device_id {
            sqlx::query(
                r#"
                SELECT d.device_id, d.first_seen_utc, d.last_seen_utc, d.hostname,
                       (SELECT COUNT(*) FROM sessions s WHERE s.device_id = d.device_id) AS session_count
                FROM devices d
                WHERE d.device_id > ?
                ORDER BY d.device_id ASC
                LIMIT ?
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
                LIMIT ?
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
            VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, 0)
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
            FROM uploads WHERE upload_id = ?
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
            finalized: row.get::<i64, _>("finalized") != 0,
        })
    }

    async fn set_upload_offset(
        &self,
        upload_id: Uuid,
        new_offset: u64,
    ) -> Result<(), StorageError> {
        let res = sqlx::query("UPDATE uploads SET offset_bytes = ? WHERE upload_id = ?")
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
        // Single conditional UPDATE makes the offset check atomic at the
        // SQLite level: two concurrent PUTs at the same offset can't both
        // succeed because only one `WHERE offset_bytes = ?` will match.
        let res = sqlx::query(
            "UPDATE uploads SET offset_bytes = ? \
             WHERE upload_id = ? AND offset_bytes = ?",
        )
        .bind(new_offset as i64)
        .bind(upload_id.to_string())
        .bind(expected_offset as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 1 {
            return Ok(true);
        }
        // 0 rows — either the upload_id doesn't exist or the offset moved.
        // Disambiguate with a cheap existence check so the handler can return
        // the right HTTP status (404 vs 409).
        let exists = sqlx::query("SELECT 1 FROM uploads WHERE upload_id = ?")
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
        let res = sqlx::query("UPDATE uploads SET finalized = 1 WHERE upload_id = ?")
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
            WHERE device_id = ? AND bundle_id = ? AND finalized = 0
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
                finalized: row.get::<i64, _>("finalized") != 0,
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
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            Err(sqlx::Error::Database(dbe))
                if dbe
                    .message()
                    .to_lowercase()
                    .contains("unique constraint failed") =>
            {
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
            FROM sessions WHERE device_id = ? AND bundle_id = ?
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
            FROM sessions WHERE session_id = ?
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
        // Keyset on (ingested_utc DESC, session_id DESC) to disambiguate ties.
        // If `before` is provided, return rows strictly older than that cursor.
        let rows = if let Some((ts, sid)) = before {
            sqlx::query(
                r#"
                SELECT session_id, device_id, bundle_id, blob_uri, content_kind,
                       size_bytes, sha256, collected_utc, ingested_utc, parse_state
                FROM sessions
                WHERE device_id = ?
                  AND (ingested_utc < ?
                       OR (ingested_utc = ? AND session_id < ?))
                ORDER BY ingested_utc DESC, session_id DESC
                LIMIT ?
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
                WHERE device_id = ?
                ORDER BY ingested_utc DESC, session_id DESC
                LIMIT ?
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

    async fn list_files_for_session(
        &self,
        session_id: Uuid,
        limit: u32,
        after_file_id: Option<&str>,
    ) -> Result<Vec<FileRow>, StorageError> {
        // Keyset on file_id ASC. UUIDv7 is time-sortable, so this gives a
        // stable insertion-order walk through the files table.
        let limit_i = limit as i64;
        let rows = if let Some(after) = after_file_id {
            sqlx::query(
                r#"
                SELECT file_id, session_id, relative_path, size_bytes,
                       format_detected, parser_kind, entry_count, parse_error_count
                FROM files
                WHERE session_id = ? AND file_id > ?
                ORDER BY file_id ASC
                LIMIT ?
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
                WHERE session_id = ?
                ORDER BY file_id ASC
                LIMIT ?
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
        // We assemble the SQL dynamically because the combinatoric count of
        // optional filters (up to 5, plus the cursor tier) is large enough
        // that writing N pre-baked variants would be worse than a small
        // string builder. Every value is still bound, not interpolated — no
        // user input reaches the SQL text.
        //
        // Ordering: `ts_ms IS NULL ASC, ts_ms ASC, entry_id ASC`.
        // In SQLite, `ts_ms IS NULL` evaluates to 0 (non-null) before 1
        // (null) under ASC, which gives us NULLS LAST semantics without a
        // dedicated clause (SQLite doesn't support NULLS LAST on indexed
        // queries anyway). Keyset cursor has to handle three cases:
        //   a) cursor.ts_ms is Some — next page starts strictly after that
        //      (ts_ms, entry_id) tuple within the non-null tier, OR anywhere
        //      in the null tier.
        //   b) cursor.ts_ms is None — we're walking the null tier; continue
        //      with entry_id > cursor.entry_id inside IS NULL rows.
        //
        // Binding order matters: we push bindings into `args` in the same
        // order their `?` placeholders appear in the SQL.
        let mut sql = String::from(
            "SELECT entry_id, file_id, line_number, ts_ms, severity, \
             component, thread, message, extras_json \
             FROM entries WHERE session_id = ?",
        );

        // Each push below appends both a SQL fragment and a binder closure
        // we'll invoke on the final query in order. This avoids the typical
        // sqlx-dynamic-query trap of needing `Box<dyn Any>` binders.
        enum Bind<'a> {
            Str(&'a str),
            OwnedStr(String),
            I64(i64),
        }
        let mut binds: Vec<Bind> = Vec::with_capacity(8);
        binds.push(Bind::OwnedStr(session_id.to_string()));

        if let Some(ref fid) = filters.file_id {
            sql.push_str(" AND file_id = ?");
            binds.push(Bind::Str(fid));
        }
        if let Some(sev) = filters.min_severity {
            sql.push_str(" AND severity >= ?");
            binds.push(Bind::I64(sev));
        }
        if let Some(after) = filters.after_ts_ms {
            // Inclusive lower bound. NULL ts_ms rows are excluded — a user
            // asking "after time X" is asking a time-sorted question, so
            // the timestamp-less tail isn't meaningful.
            sql.push_str(" AND ts_ms IS NOT NULL AND ts_ms >= ?");
            binds.push(Bind::I64(after));
        }
        if let Some(before) = filters.before_ts_ms {
            // Exclusive upper bound. Same NULL-exclusion rationale as above.
            sql.push_str(" AND ts_ms IS NOT NULL AND ts_ms < ?");
            binds.push(Bind::I64(before));
        }
        if let Some(ref q) = filters.q_like {
            sql.push_str(" AND message LIKE ?");
            binds.push(Bind::Str(q));
        }
        if let Some(ref c) = filters.cursor {
            match c.ts_ms {
                Some(ts) => {
                    // Non-null tier continuation: advance past (ts, entry_id)
                    // but stay in the non-null rows, OR drop into the null
                    // tail (which orders after every non-null row).
                    sql.push_str(
                        " AND ( \
                           (ts_ms IS NOT NULL AND (ts_ms > ? OR (ts_ms = ? AND entry_id > ?))) \
                           OR ts_ms IS NULL \
                         )",
                    );
                    binds.push(Bind::I64(ts));
                    binds.push(Bind::I64(ts));
                    binds.push(Bind::I64(c.entry_id));
                }
                None => {
                    // Null tier continuation: stay in IS NULL rows, advance
                    // past entry_id.
                    sql.push_str(" AND ts_ms IS NULL AND entry_id > ?");
                    binds.push(Bind::I64(c.entry_id));
                }
            }
        }

        sql.push_str(" ORDER BY (ts_ms IS NULL) ASC, ts_ms ASC, entry_id ASC LIMIT ?");
        binds.push(Bind::I64(limit as i64));

        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = match b {
                Bind::Str(s) => q.bind(*s),
                Bind::OwnedStr(s) => q.bind(s.as_str()),
                Bind::I64(n) => q.bind(*n),
            };
        }
        let rows = q.fetch_all(&self.pool).await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(EntryRow {
                entry_id: r.get::<i64, _>("entry_id"),
                file_id: r.get::<String, _>("file_id"),
                // SQLite stores as INTEGER; cast down via i64 → u32.
                line_number: r.get::<i64, _>("line_number") as u32,
                ts_ms: r.get::<Option<i64>, _>("ts_ms"),
                severity: r.get::<i64, _>("severity"),
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

    #[tokio::test]
    async fn migrations_apply_and_upsert_device_works() {
        let store = SqliteMetadataStore::connect(":memory:").await.unwrap();
        let now = Utc::now();
        store.upsert_device("WIN-1", Some("lab01"), now).await.unwrap();
        store.upsert_device("WIN-1", None, now).await.unwrap();
        let rows = store.list_devices(10, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].device_id, "WIN-1");
        assert_eq!(rows[0].hostname.as_deref(), Some("lab01"));
        assert_eq!(rows[0].session_count, 0);
    }
}
