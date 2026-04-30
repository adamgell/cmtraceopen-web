//! Mine a Postgres SQL dump into a `shape.json`. Loads the dump into a
//! scratch local PG (via env DATABASE_URL or --pg-url), runs the analytic
//! queries, dumps the result. One-shot tool; the operator runs it once
//! when they have a fresh dump and commits the resulting JSON.

use clap::Parser;
use parse_load_harness::corpus::shape::{PercentileBucket, Shape};
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Parser, Debug)]
struct Args {
    /// Path to the .sql dump file.
    sql_file: PathBuf,
    #[arg(long, default_value = "shape.json")]
    out: PathBuf,
    /// Database URL of a scratch Postgres the dump will be loaded into.
    /// The script DROPs the schema first.
    #[arg(long, env = "DATABASE_URL")]
    pg_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let pool = PgPoolOptions::new().connect(&args.pg_url).await?;

    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
        .execute(&pool)
        .await?;
    drop(pool);

    let status = Command::new("psql")
        .args(["-d", &args.pg_url, "-f", args.sql_file.to_str().unwrap()])
        .status()
        .await?;
    anyhow::ensure!(status.success(), "psql -f {} failed", args.sql_file.display());

    let pool = PgPoolOptions::new().connect(&args.pg_url).await?;

    let files_per_bundle = bucket_query(
        &pool,
        "SELECT count(*)::float8 FROM files GROUP BY session_id",
    )
    .await?;
    let bytes_per_file = bucket_query(
        &pool,
        "SELECT size_bytes::float8 FROM files",
    )
    .await?;
    let parser_kind_weights = weight_query(
        &pool,
        "SELECT parser_kind, count(*)::float8 FROM files WHERE parser_kind IS NOT NULL GROUP BY parser_kind",
    )
    .await?;
    let fallback_rate_per_kb = fallback_query(&pool).await?;
    let encoding_weights = BTreeMap::from([
        ("Utf8".to_string(), 0.86),
        ("Utf16Le".to_string(), 0.13),
        ("Other".to_string(), 0.01),
    ]); // Encoding isn't recorded in DB; carry over the default.

    let shape = Shape {
        files_per_bundle,
        bytes_per_file,
        parser_kind_weights,
        fallback_rate_per_kb,
        encoding_weights,
    };
    let bytes = serde_json::to_vec_pretty(&shape)?;
    tokio::fs::write(&args.out, bytes).await?;
    eprintln!("wrote {}", args.out.display());
    Ok(())
}

async fn bucket_query(pool: &sqlx::PgPool, sql: &str) -> anyhow::Result<PercentileBucket> {
    let rows: Vec<(f64,)> = sqlx::query_as(sql).fetch_all(pool).await?;
    let mut v: Vec<f64> = rows.into_iter().map(|(x,)| x).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return Ok(PercentileBucket { p50: 0, p90: 0, p99: 0, max_observed: 0 });
    }
    let pick = |q: f64| v[((v.len() - 1) as f64 * q) as usize] as u64;
    Ok(PercentileBucket {
        p50: pick(0.50),
        p90: pick(0.90),
        p99: pick(0.99),
        max_observed: *v.last().unwrap() as u64,
    })
}

async fn weight_query(pool: &sqlx::PgPool, sql: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let rows: Vec<(String, f64)> = sqlx::query_as(sql).fetch_all(pool).await?;
    let total: f64 = rows.iter().map(|(_, c)| c).sum();
    Ok(rows.into_iter().map(|(k, c)| (k, c / total)).collect())
}

async fn fallback_query(pool: &sqlx::PgPool) -> anyhow::Result<BTreeMap<String, f64>> {
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT parser_kind, sum(parse_error_count)::float8, sum(size_bytes)::float8
         FROM files
         WHERE parser_kind IS NOT NULL
         GROUP BY parser_kind",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(k, errs, sz)| {
            let kb = (sz / 1024.0).max(1.0);
            (k, errs / kb)
        })
        .collect())
}
