//! The schedule worker. Runs on the leader replica only.

use crate::schedule::{leader, rotation, selector};
use crate::state::AppState;
use crate::storage::{BundleRequestSource, NewBundleRequest};
use chrono::{DateTime, Utc};
use croner::Cron;
use serde_json::Value;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ScheduleRow {
    name: String,
    cron: String,
    selector_json: Value,
    rate_pct: i32,
    jitter_seconds: i32,
    cooldown_seconds: i32,
    last_fired_at: Option<DateTime<Utc>>,
    // Fetched from the DB so sqlx::FromRow maps it; the worker uses it to
    // decide which rows are due (WHERE next_fire_at <= now()) rather than
    // reading it in Rust — hence the allow.
    #[allow(dead_code)]
    next_fire_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

pub async fn run(state: Arc<AppState>, pool: Pool<Postgres>) {
    loop {
        let mut leader_conn = match leader::try_acquire(&pool).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Err(e) => {
                warn!(error = %e, "leader acquire failed");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };
        info!("became schedule leader");
        loop {
            let due: Result<Vec<ScheduleRow>, _> = sqlx::query_as(
                "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                        cooldown_seconds, last_fired_at, next_fire_at, created_at
                 FROM schedules
                 WHERE enabled AND next_fire_at <= now()
                 ORDER BY next_fire_at ASC LIMIT 16",
            )
            .fetch_all(&mut *leader_conn)
            .await;
            match due {
                Ok(rows) => {
                    for s in rows {
                        if let Err(e) = fire(&state, &pool, &s).await {
                            warn!(schedule = %s.name, error = %e, "fire_schedule failed");
                        }
                        if let Err(e) = update_next_fire(&mut leader_conn, &s).await {
                            warn!(schedule = %s.name, error = %e, "update_next_fire failed");
                            break;
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "scan schedules failed; demoting");
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
}

async fn fire(
    state: &Arc<AppState>,
    pool: &Pool<Postgres>,
    s: &ScheduleRow,
) -> anyhow::Result<()> {
    let (where_clause, args) = selector::render(&s.selector_json)?;
    let sql = format!("SELECT device_id FROM agents WHERE {where_clause}");
    let rows: Vec<(String,)> = sqlx::query_as_with(&sql, args).fetch_all(pool).await?;
    let device_ids: Vec<String> = rows.into_iter().map(|(id,)| id).collect();
    let total = device_ids.len();

    let salt_seed = s.last_fired_at.unwrap_or(s.created_at);
    let salt_value = rotation::salt(salt_seed);
    let k = rotation::target_count(total, s.rate_pct as u32);
    let picked = rotation::pick(&device_ids, k, salt_value);

    let cooldown = s.cooldown_seconds as i64;
    let kept: Vec<String> = if cooldown <= 0 {
        picked
    } else {
        let mut kept = Vec::with_capacity(picked.len());
        for id in picked {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1::bigint FROM bundle_requests
                 WHERE device_id = $1 AND requested_at > now() - $2 * interval '1 second'
                 LIMIT 1",
            )
            .bind(&id)
            .bind(cooldown)
            .fetch_optional(pool)
            .await?;
            if row.is_none() {
                kept.push(id);
            }
        }
        kept
    };

    if kept.is_empty() {
        info!(schedule = %s.name, "no candidates after cooldown");
        return Ok(());
    }

    use rand::Rng;
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(salt_seed.timestamp() as u64);
    for id in kept {
        let request_id = Uuid::new_v4();
        let row = NewBundleRequest {
            request_id,
            device_id: id.clone(),
            source: BundleRequestSource::Scheduled,
            schedule_name: Some(s.name.clone()),
            operator_email: None,
            requested_at: Utc::now(),
        };
        state.meta.insert_bundle_request(row).await?;
        let jitter_ms = if s.jitter_seconds > 0 {
            rng.random_range(0..(s.jitter_seconds as u64 * 1000))
        } else {
            0
        };
        let state2 = state.clone();
        let schedule_name = s.name.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
            crate::schedule::dispatch::dispatch_scheduled_request(
                &state2,
                &id,
                request_id,
                &schedule_name,
            )
            .await;
        });
    }
    Ok(())
}

async fn update_next_fire(
    conn: &mut sqlx::pool::PoolConnection<Postgres>,
    s: &ScheduleRow,
) -> anyhow::Result<()> {
    let cron = Cron::new(&s.cron).parse()?;
    let next = cron.find_next_occurrence(&Utc::now(), false)?;
    sqlx::query("UPDATE schedules SET last_fired_at = now(), next_fire_at = $1 WHERE name = $2")
        .bind(next)
        .bind(&s.name)
        .execute(&mut **conn)
        .await?;
    Ok(())
}
