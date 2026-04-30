//! Periodically marks/cleans `connections` rows for devices whose last
//! heartbeat is too stale. Two stages:
//!
//!  - heartbeat older than HEARTBEAT_TIMEOUT (90s) but disconnected_at is
//!    NULL → set disconnected_at = now()
//!  - disconnected_at older than 5 minutes → DELETE the row
//!
//! Runs every 30s on every replica (idempotent).

use sqlx::PgPool;
use std::time::Duration;
use tracing::warn;

pub async fn run(pool: PgPool) {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = sqlx::query(
            "UPDATE connections SET disconnected_at = now()
             WHERE disconnected_at IS NULL
               AND last_heartbeat_at < now() - interval '90 seconds'"
        ).execute(&pool).await {
            warn!(error = %e, "sweeper: mark-stale step failed");
        }
        if let Err(e) = sqlx::query(
            "DELETE FROM connections
             WHERE disconnected_at IS NOT NULL
               AND disconnected_at < now() - interval '5 minutes'"
        ).execute(&pool).await {
            warn!(error = %e, "sweeper: delete-stale step failed");
        }
    }
}
