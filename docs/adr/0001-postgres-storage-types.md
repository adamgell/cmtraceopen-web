# ADR 0001: Postgres storage types

> Status: **Accepted** (Wave 4 P1.1)
> Date: 2026-04-22
> Supersedes: —
> Superseded by: —
> Cross-links: PR #77 (Postgres backend), PR #79 (audit log), issue #110 (audit hash chain)

## Context

PR #77 introduces a Postgres metadata-store backend alongside the existing
SQLite store. The two backends share a common Rust trait
(`MetadataStore` / `AuditStore`), but the underlying SQL columns can use
different native types per engine. The PR opted for the most portable
column types — `TEXT` for timestamps, `TEXT` for JSON payloads, `TEXT` for
UUIDs — so the same Rust serialisation code paths work unchanged across
both engines.

The reviewer flagged this as a meaningful long-term tradeoff:

> "Storing `ts_utc TEXT` in Postgres means: no `TIMESTAMPTZ` range
> queries, no `now() - interval '7 days'` operator-friendliness,
> lexicographic sort relies on RFC-3339 zero-padding (which
> `chrono::DateTime::to_rfc3339` does emit, but a future code change
> could regress). For a long-running prod system this is a meaningful
> tradeoff."

This ADR records the design call so the tradeoff is captured at the
right granularity (storage-engine selection, not per-column hand-wavy
choice) and the follow-up migration is unambiguous.

## Decision

For **Wave 4 P1.1** (this PR):

- `ts_utc` columns: **TEXT** (RFC-3339 / ISO-8601 strings).
- JSON payload columns (`labels_json`, `details_json`, etc.): **TEXT**.
- UUID columns: **TEXT**.

The portability benefit (single `chrono::DateTime` / `serde_json::Value` /
`uuid::Uuid` binding code path across SQLite + Postgres) outweighs the
loss of native PG operator support at this scale.

For **the follow-up migration** (post-Wave-4, separate PR):

- `ts_utc` columns: migrate to **`TIMESTAMPTZ`** in a Postgres-only
  `ALTER COLUMN ... USING ts_utc::timestamptz` step. This unlocks
  `now() - interval '7 days'`, `BETWEEN`, range partitioning, and
  index-friendly time-range queries against the `audit_log`,
  `sessions`, and `entries` tables.
- JSON payload columns: migrate to **`JSONB`** so operators can run
  `WHERE details_json->>'reason' = 'manual_disable'` style queries
  against the audit log without `regexp_replace` hacks.
- UUID columns: migrate to **`UUID`** for 16-byte storage + native
  ordering. Modest space + I/O win at scale; not load-bearing on
  correctness.

The follow-up does NOT touch SQLite — SQLite has no native
`TIMESTAMPTZ` / `JSONB` / `UUID` and would gain nothing from the
type-narrowing. SQLite continues with `TEXT`.

## Consequences

**Positive (now):**
- Single Rust serialisation path across both engines. Less surface area
  for backend-specific parsing bugs.
- Migration tree drift is bounded (`migrations/` and `migrations-pg/`
  diverge only on `INTEGER` vs `BIGINT` and `IF NOT EXISTS` — see PR
  #77 review comment A and the `xtask migrations-parity` follow-up).
- Insert hot path is identical — no `to_rfc3339` → `parse::<TIMESTAMPTZ>`
  per-bind cost.

**Negative (now):**
- Operators querying the Postgres backend cannot use
  `now() - interval '7 days'` against `ts_utc`; they must compare to
  an RFC-3339 string (`ts_utc > '2026-04-15T00:00:00Z'`). Lexicographic
  sort works only because `chrono::DateTime::to_rfc3339` emits
  zero-padded fields — a regression in chrono or our binding code
  could quietly break ordering.
- No native JSON path queries on Postgres. Operators wanting to query
  inside `details_json` / `labels_json` columns must use SQL string
  matching or read the column out and parse in code.
- Larger storage footprint for UUIDs (36 bytes TEXT vs 16 bytes
  native).

**Migration plan (follow-up PR):**
- File: `crates/api-server/migrations-pg/0004_native_types.sql`
  (numbered to follow whichever audit-log migration lands; coordinate
  with PR #79).
- Run `ALTER COLUMN ts_utc TYPE TIMESTAMPTZ USING ts_utc::timestamptz`
  on every `ts_utc` column in scope.
- Run `ALTER COLUMN <jsoncol> TYPE JSONB USING <jsoncol>::jsonb` on
  every `*_json` column.
- UUIDs are last — operators can defer this if the I/O win is
  marginal at their scale.
- Update `meta_postgres.rs` to bind `DateTime<Utc>` directly via
  sqlx's TIMESTAMPTZ codec (which is already in the sqlx feature set
  we use), and update the relevant `WHERE` / `ORDER BY` clauses.

## Cross-links

- PR #77 — Postgres backend foundation (this PR).
- PR #79 — Audit log; `audit_log.ts_utc` and `audit_log.details_json`
  inherit this decision and will be migrated together with the rest in
  the follow-up.
- Issue #110 — Audit-log tamper evidence (hash chain). The hash-chain
  follow-up should land BEFORE the TIMESTAMPTZ / JSONB migration so
  the migration script knows the exact `audit_log` schema to alter.
