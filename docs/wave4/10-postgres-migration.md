# 10 — Postgres Migration: Portable Schema + Dual-Backend Storage

**Wave 4 — P1.1**  
**Status**: Implemented  
**Date**: 2026-04-22

---

## Overview

This document explains the approach taken to make `api-server` able to run
against **PostgreSQL** (Azure Database for PostgreSQL Flexible Server) in
addition to the existing **SQLite** backend, per the requirements in issue
P1.1.

---

> **Storage-type design call**: see ADR
> [`docs/adr/0001-postgres-storage-types.md`](../adr/0001-postgres-storage-types.md)
> for the recorded TEXT-now / TIMESTAMPTZ-+-JSONB-as-follow-up decision.
> Cross-linked from PR #79 (audit log) so the future migration covers all
> tables consistently.

## Design Decisions

### 1. Separate Migration Directories (not a single portable schema)

We considered two approaches:

| Approach | Pro | Con |
|---|---|---|
| Single portable schema (rewrite SQLite migrations) | One set of files to maintain | `AUTOINCREMENT` vs `BIGSERIAL` is not portable in any SQL dialect; requires dialect detection at migration runtime |
| Separate dirs: `migrations/` (SQLite) + `migrations-pg/` (Postgres) | Correct idiomatic SQL for each engine; sqlx `migrate!()` macro picks the right dir at compile time | Two files to keep in sync for schema changes |

**Choice: separate directories.** The only truly non-portable construct is
the `entries` table auto-increment primary key:

- SQLite: `entry_id INTEGER PRIMARY KEY AUTOINCREMENT`
- Postgres: `entry_id BIGSERIAL PRIMARY KEY`

Everything else (`TEXT`, `INTEGER`, `BIGINT`, `ON CONFLICT`, indexes) is
standard SQL:2003 that works in both engines without modification.

#### Schema differences (`migrations/` vs `migrations-pg/`)

| SQLite (`migrations/`) | Postgres (`migrations-pg/`) | Notes |
|---|---|---|
| `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGSERIAL PRIMARY KEY` | entries.entry_id auto-increment |
| `INTEGER` for size_bytes/offset | `BIGINT` | Prevents 32-bit overflow on large bundles |
| `(ts_ms IS NULL) ASC` NULLS-LAST trick | `ORDER BY ts_ms ASC NULLS LAST` | Postgres supports SQL:2003 NULLS LAST natively |
| `TEXT` timestamps | `TEXT` timestamps | Kept identical to avoid changing all bind sites |

**Timestamps as TEXT**: Both backends store timestamps as ISO-8601 / RFC-3339
strings. This is intentional — switching to `TIMESTAMPTZ` in Postgres would
require changing every bind site in `meta_postgres.rs` to use
`sqlx::types::time::OffsetDateTime` or `chrono::DateTime<Utc>` instead of
strings, adding non-trivial surface area. The lexicographic ordering of
zero-padded RFC-3339 strings is identical to their temporal ordering, so
range queries and `ORDER BY ingested_utc DESC` work correctly.

**`extras_json` as TEXT**: The issue notes "Postgres JSONB vs SQLite TEXT —
pick one". We chose `TEXT` for both because:
- `extras_json` is only ever written once (at parse time) and read back as a
  raw string for the HTTP response. No server-side JSON operators are used.
- Using `JSONB` would require switching the bind type to `sqlx::types::Json<T>`,
  adding a round-trip `serde_json::Value` parse on every insert — overhead
  with no benefit given the current query patterns.
- Staying with `TEXT` keeps `meta_postgres.rs` and `meta_sqlite.rs` more
  structurally similar, making future cross-cutting changes easier to apply.

---

### 2. Cargo Feature Flags

```
sqlite  = ["sqlx/sqlite", "sqlx/tls-none"]
postgres = ["sqlx/postgres", "sqlx/tls-rustls-aws-lc-rs"]
```

- `sqlite` is in the **default** feature set alongside `mtls`, `crl`, and
  `azure`. Existing deployments need no config changes.
- `postgres` is opt-in. The Docker image (used for docker-compose and Azure
  ACA) is built with `--features postgres` so it can target both the sqlite
  fallback and Postgres.
- `tls-none` (sqlite) and `tls-rustls-aws-lc-rs` (postgres) can coexist in
  sqlx 0.8: the rustls TLS connector handles Postgres network connections;
  SQLite is a file driver that never opens a network socket and ignores the
  TLS layer entirely.
- The binary already depends on `aws-lc-rs` transitively (via `rustls`,
  `reqwest`, and the `mtls` feature). Adding `tls-rustls-aws-lc-rs` to sqlx
  reuses that existing C build; it does not introduce a new dependency.

#### Build matrix

| Command | Use case |
|---|---|
| `cargo build -p api-server` | Dev (sqlite, all defaults) |
| `cargo build -p api-server --features postgres` | Postgres-enabled binary |
| `cargo build -p api-server --no-default-features --features sqlite` | Minimal sqlite-only binary (no C TLS in sqlx network drivers) |
| `docker buildx build -f crates/api-server/Dockerfile .` | Container image (sqlite + postgres) |

---

### 3. Connection-String Factory (`build_metadata_store`)

`crates/api-server/src/storage/mod.rs` now exports:

```rust
pub async fn build_metadata_store(config: &Config) -> Result<Arc<dyn MetadataStore>, BuildMetadataStoreError>
```

The factory reads `config.database_url` and dispatches on the scheme:

- `postgres://…` or `postgresql://…` → `PgMetadataStore` (requires `postgres` feature)
- anything else → `SqliteMetadataStore` (requires `sqlite` feature)

`Config.database_url` is populated in `Config::from_env()`:
1. If `CMTRACE_DATABASE_URL` is set and non-empty, use it directly.
2. Otherwise, synthesise `sqlite://<CMTRACE_SQLITE_PATH>` (or `sqlite::memory:`
   for `:memory:`). This means deployments that only set `CMTRACE_SQLITE_PATH`
   (or rely on its default) continue to work unchanged.

---

### 4. `PgMetadataStore` (meta_postgres.rs)

Mirrors `SqliteMetadataStore` method-for-method but targets `PgPool`:

- **Positional placeholders**: Postgres requires `$1`, `$2`, … instead of `?`.
  The dynamic `query_entries` method tracks a `param_idx` counter and
  formats `$N` tokens into the SQL string, then binds in the same order.
- **Unique-constraint detection**: SQLite returns `UNIQUE constraint failed`
  in the error message. Postgres returns error code `23505`. The
  `insert_session` method checks `dbe.code() == Some("23505")` instead of
  the message string.
- **Integer widths on read**: Postgres `INTEGER` columns return `i32` from
  sqlx; SQLite `INTEGER` columns return `i64`. `meta_postgres.rs` casts
  `i32 → u32` for `line_number` and `i32 → i64` for `severity` to match the
  shared `EntryRow` struct.
- **BIGSERIAL entry_id**: Postgres `BIGSERIAL` still surfaces as `i64` in
  Rust, matching the `EntryRow.entry_id: i64` field.
- **Pool stats**: `PgPool::num_idle()` returns `usize`; capped to `u32::MAX`
  via `try_from`.

---

### 5. Postgres-Specific Tests

Unit tests in `meta_postgres.rs` are gated on `CMTRACE_DATABASE_URL` pointing
at a live Postgres instance:

```rust
fn pg_url() -> Option<String> {
    std::env::var("CMTRACE_DATABASE_URL")
        .ok()
        .filter(|u| u.starts_with("postgres://") || u.starts_with("postgresql://"))
}
```

If the variable is not set (or is a sqlite URL), the tests print a skip
message and return early. This lets the default CI run (`cargo test
-p api-server`) pass without Docker, while a full validation run can target
a real Postgres by setting `CMTRACE_DATABASE_URL` before invoking `cargo test
-p api-server --features postgres`.

---

## Deploy Recipe

### Local dev (docker compose)

```sh
# Postgres + api-server in one command
docker compose up --build

# Verify
curl http://localhost:8080/healthz
curl http://localhost:8080/
```

The compose file sets `CMTRACE_DATABASE_URL=postgres://cmtrace:cmtrace@postgres:5432/cmtrace`
and `depends_on` ensures Postgres is healthy before the api-server starts.

### SQLite (original path)

```sh
CMTRACE_DATABASE_URL=sqlite:./data/meta.sqlite cargo run -p api-server
# or, using the legacy env var (still supported):
CMTRACE_SQLITE_PATH=./data/meta.sqlite cargo run -p api-server
```

### Azure Container Apps (Postgres)

Set in the ACA environment (see `docs/wave4/05-azure-deploy.md`):

```
CMTRACE_DATABASE_URL=postgres://cmtrace:<password>@<pg-server>.postgres.database.azure.com:5432/cmtrace?sslmode=require
```

The binary must be built with `--features postgres` (the Dockerfile already
does this). Azure Postgres Flexible Server enforces TLS; the
`tls-rustls-aws-lc-rs` sqlx feature handles the SSL handshake with the
server's publicly-trusted certificate.

---

## Gotchas

1. **`tls-none` + `tls-rustls-aws-lc-rs` coexistence**: Both can be active in
   the same build (e.g. when building with defaults + `--features postgres`).
   sqlx 0.8 uses the feature flags per-driver: the SQLite driver (file-based)
   never touches the TLS layer; the Postgres driver uses rustls. No conflict.

2. **`i32` vs `i64` column reads**: Postgres `INTEGER` = 32-bit; SQLite
   `INTEGER` = variable-width, always decoded as `i64` by sqlx-sqlite. Any
   new column added to `entries` or `files` with integer semantics must be
   `BIGINT` in `migrations-pg/` to get consistent `i64` reads in both
   backends, OR the read cast must be explicit (`as u32`, `as u64`, etc.).

3. **Unique-violation detection**: Always compare `dbe.code()`, not
   `dbe.message()`. Postgres error codes are standardised (SQLSTATE `23505`
   for unique violation); messages vary by locale and server version.

4. **Azure Postgres SSL**: Azure Postgres Flexible Server requires TLS by
   default (cannot be turned off in the Flexible Server tier). Append
   `?sslmode=require` to the connection string if the driver doesn't negotiate
   TLS automatically. With `tls-rustls-aws-lc-rs`, sqlx will attempt TLS; the
   server's certificate is signed by the DigiCert chain (publicly trusted),
   so no custom CA bundle is needed on the client side.

5. **Schema version divergence**: `migrations/` and `migrations-pg/` are
   independently versioned. If a new schema migration is needed, add a new
   file to **both** directories with the same logical change expressed in
   each engine's syntax. The sqlx migration checksum is computed per-dir, so
   a file present in one dir but not the other will not cause an error on the
   other engine (each engine only knows about its own migrations).

---

## Cross-references

- `docs/wave4/05-azure-deploy.md` — provisions the Postgres Flexible Server
  this targets
- `docs/wave4/04-day2-operations.md` §4 — "SQLite caps at ~250 devices"
  capacity threshold note that motivates this migration
- `crates/api-server/src/storage/meta_sqlite.rs` — SQLite implementation
- `crates/api-server/src/storage/meta_postgres.rs` — Postgres implementation
- `crates/api-server/migrations/` — SQLite migrations
- `crates/api-server/migrations-pg/` — Postgres migrations
