//! Postgres advisory-lock leader election. The lock is session-scoped, so
//! the leader holds a dedicated PgConnection for its entire lifetime and
//! runs all leader work on that connection.

use sqlx::{pool::PoolConnection, PgPool, Postgres};

pub const SCHEDULE_LEADER_KEY: i64 = 0x434d54_5343484544; // "CMTSCHED"

/// Try to become the leader. Returns the held connection on success, or
/// None if another replica is already leader.
pub async fn try_acquire(pool: &PgPool) -> sqlx::Result<Option<PoolConnection<Postgres>>> {
    let mut conn = pool.acquire().await?;
    let acquired: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(SCHEDULE_LEADER_KEY)
        .fetch_one(&mut *conn)
        .await?;
    if acquired.0 {
        Ok(Some(conn))
    } else {
        Ok(None)
    }
}
