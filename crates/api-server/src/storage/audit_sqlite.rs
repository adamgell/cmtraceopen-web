//! SQLite-backed [`AuditStore`] — append-only audit log for admin actions.
//!
//! The implementation lives in its own file to keep `meta_sqlite.rs` focused
//! on the core metadata operations.  Both modules share the same `SqlitePool`
//! through [`SqliteMetadataStore::audit_store`], which clones the pool handle
//! (cheap Arc bump) rather than opening a second connection.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{AuditFilters, AuditRow, AuditStore, NewAuditRow, StorageError};

/// SQLite-backed [`AuditStore`].
///
/// Constructed by [`SqliteMetadataStore::audit_store`] so the audit log
/// shares the existing connection pool and benefits from WAL-mode writes
/// already configured on the pool.
pub struct AuditSqliteStore {
    pool: SqlitePool,
}

impl AuditSqliteStore {
    /// Build from an existing pool. Intended to be called via
    /// [`SqliteMetadataStore::audit_store`] only.
    pub(super) fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(s).map_err(|e| {
        StorageError::Sqlx(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid uuid in audit_log: {e}"),
        ))))
    })
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            StorageError::Sqlx(sqlx::Error::Decode(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid timestamp in audit_log: {e}"),
            ))))
        })
}

#[async_trait]
impl AuditStore for AuditSqliteStore {
    async fn insert_audit_row(&self, row: NewAuditRow) -> Result<(), StorageError> {
        let id_s = row.id.to_string();
        let ts_s = row.ts_utc.to_rfc3339();
        let request_id_s = row.request_id.map(|u| u.to_string());

        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, ts_utc, principal_kind, principal_id, principal_display,
                 action, target_kind, target_id, result, details_json, request_id)
            VALUES
                (?, ?, ?, ?, ?,  ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id_s)
        .bind(&ts_s)
        .bind(&row.principal_kind)
        .bind(&row.principal_id)
        .bind(&row.principal_display)
        .bind(&row.action)
        .bind(&row.target_kind)
        .bind(&row.target_id)
        .bind(&row.result)
        .bind(&row.details_json)
        .bind(&request_id_s)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_audit_rows(
        &self,
        filters: &AuditFilters,
        limit: u32,
    ) -> Result<Vec<AuditRow>, StorageError> {
        // Cap at 1000 so a misconfigured caller can't pull the whole table in
        // one shot.
        let effective_limit = limit.min(1000) as i64;

        // Build the query dynamically depending on which filters are set.
        // SQLite has no parameterised WHERE…AND construction, so we
        // assemble the SQL string. All values are still bound as parameters
        // so there is no SQL-injection risk.
        let after_ts_s = filters.after_ts.as_ref().map(|dt| dt.to_rfc3339());

        let rows = match (&after_ts_s, &filters.principal, &filters.action) {
            (None, None, None) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(after), None, None) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE ts_utc > ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(after)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(principal), None) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE principal_id = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(principal)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (None, None, Some(action)) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE action = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(action)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(after), Some(principal), None) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE ts_utc > ? AND principal_id = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(after)
                .bind(principal)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(after), None, Some(action)) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE ts_utc > ? AND action = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(after)
                .bind(action)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(principal), Some(action)) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE principal_id = ? AND action = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(principal)
                .bind(action)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(after), Some(principal), Some(action)) => {
                sqlx::query(
                    "SELECT id, ts_utc, principal_kind, principal_id, principal_display,
                            action, target_kind, target_id, result, details_json, request_id
                     FROM audit_log
                     WHERE ts_utc > ? AND principal_id = ? AND action = ?
                     ORDER BY ts_utc DESC
                     LIMIT ?",
                )
                .bind(after)
                .bind(principal)
                .bind(action)
                .bind(effective_limit)
                .fetch_all(&self.pool)
                .await?
            }
        };

        rows.into_iter()
            .map(|row| {
                use sqlx::Row;
                let id_s: String = row.try_get("id")?;
                let ts_s: String = row.try_get("ts_utc")?;
                let request_id_s: Option<String> = row.try_get("request_id")?;
                Ok(AuditRow {
                    id: parse_uuid(&id_s)?,
                    ts_utc: parse_ts(&ts_s)?,
                    principal_kind: row.try_get("principal_kind")?,
                    principal_id: row.try_get("principal_id")?,
                    principal_display: row.try_get("principal_display")?,
                    action: row.try_get("action")?,
                    target_kind: row.try_get("target_kind")?,
                    target_id: row.try_get("target_id")?,
                    result: row.try_get("result")?,
                    details_json: row.try_get("details_json")?,
                    request_id: request_id_s.as_deref().map(parse_uuid).transpose()?,
                })
            })
            .collect()
    }
}
