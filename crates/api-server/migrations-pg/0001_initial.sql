-- Postgres-targeted schema for Phase 3 M1 MVP.
--
-- Portable notes vs the SQLite version (migrations/0001_initial.sql):
--   * TEXT PRIMARY KEY — identical in both engines.
--   * INTEGER NOT NULL / INTEGER NOT NULL DEFAULT 0 — both accept standard
--     SQL INTEGER; Postgres stores as int4 (32-bit), SQLite as a variable-
--     width integer. All Rust bindings use i64 / i32 casts, so this is safe.
--   * Timestamps stored as TEXT (ISO-8601 / RFC-3339). Keeping TEXT lets the
--     same chrono serialisation/deserialisation logic run unchanged across
--     both backends. TIMESTAMPTZ would be more idiomatic Postgres but would
--     require re-working every bind site in meta_postgres.rs.
--   * ON CONFLICT DO UPDATE — standard SQL:2003 MERGE semantics; supported
--     identically in SQLite ≥ 3.24 and PostgreSQL ≥ 9.5.
--   * REFERENCES (FK) — enforced by default in Postgres; SQLite requires the
--     per-connection `PRAGMA foreign_keys = ON` pragma. The Postgres backend
--     relies on the database-level FK enforcement.

CREATE TABLE IF NOT EXISTS devices (
  device_id        TEXT PRIMARY KEY,
  cert_fingerprint TEXT UNIQUE,          -- nullable until mTLS lands (M2)
  first_seen_utc   TEXT NOT NULL,
  last_seen_utc    TEXT NOT NULL,
  hostname         TEXT,
  os_version       TEXT,
  enrollment_state TEXT,
  labels_json      TEXT
);

CREATE TABLE IF NOT EXISTS uploads (
  upload_id       TEXT PRIMARY KEY,
  bundle_id       TEXT NOT NULL,
  device_id       TEXT NOT NULL,
  size_bytes      BIGINT NOT NULL,
  expected_sha256 TEXT NOT NULL,
  content_kind    TEXT NOT NULL,
  offset_bytes    BIGINT NOT NULL DEFAULT 0,
  staged_path     TEXT NOT NULL,
  created_utc     TEXT NOT NULL,
  finalized       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_uploads_device_bundle ON uploads(device_id, bundle_id);

CREATE TABLE IF NOT EXISTS sessions (
  session_id     TEXT PRIMARY KEY,
  device_id      TEXT NOT NULL REFERENCES devices(device_id),
  bundle_id      TEXT NOT NULL,
  blob_uri       TEXT NOT NULL,
  content_kind   TEXT NOT NULL,
  size_bytes     BIGINT NOT NULL,
  sha256         TEXT NOT NULL,
  collected_utc  TEXT,
  ingested_utc   TEXT NOT NULL,
  parse_state    TEXT NOT NULL DEFAULT 'pending',
  UNIQUE(device_id, bundle_id)
);

CREATE INDEX IF NOT EXISTS idx_sessions_device_ingested ON sessions(device_id, ingested_utc DESC);
