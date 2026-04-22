-- Phase 3 M2: per-session parsed entries — Postgres version.
--
-- Key differences from the SQLite version (migrations/0002_entries.sql):
--   * entry_id: SQLite uses INTEGER PRIMARY KEY AUTOINCREMENT (rowid alias).
--     Postgres uses BIGSERIAL which maps to `CREATE SEQUENCE` + `DEFAULT
--     nextval(...)`. Both surface as an i64 in Rust via sqlx.
--   * INTEGER columns (size_bytes, entry_count, etc.) use BIGINT in Postgres
--     to match the i64 Rust type and avoid potential overflow on large bundles.
--     SQLite's INTEGER is already variable-width so this difference is only
--     at the DDL level.
--   * NULLS LAST ordering: Postgres supports the SQL:2003 `NULLS LAST`
--     clause directly in ORDER BY. The SQLite version relies on the
--     `(ts_ms IS NULL) ASC` trick because SQLite lacks native NULLS LAST on
--     indexed columns. The Postgres query path in meta_postgres.rs uses the
--     explicit `NULLS LAST` clause for clarity.

CREATE TABLE IF NOT EXISTS files (
  file_id           TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES sessions(session_id),
  relative_path     TEXT NOT NULL,
  size_bytes        BIGINT NOT NULL,
  format_detected   TEXT,
  parser_kind       TEXT,
  entry_count       BIGINT NOT NULL DEFAULT 0,
  parse_error_count BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS entries (
  -- BIGSERIAL is the Postgres equivalent of SQLite's INTEGER PRIMARY KEY
  -- AUTOINCREMENT. Both are monotonically increasing 64-bit integers.
  entry_id    BIGSERIAL PRIMARY KEY,
  session_id  TEXT NOT NULL,
  file_id     TEXT NOT NULL REFERENCES files(file_id),
  line_number INTEGER NOT NULL,
  ts_ms       BIGINT,            -- nullable: not every parsed entry carries a timestamp
  severity    INTEGER NOT NULL,  -- 0=Info, 1=Warning, 2=Error
  component   TEXT,
  thread      TEXT,
  message     TEXT NOT NULL,
  extras_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_entries_session_ts ON entries(session_id, ts_ms);
CREATE INDEX IF NOT EXISTS idx_entries_session_severity_ts ON entries(session_id, severity, ts_ms);
CREATE INDEX IF NOT EXISTS idx_files_session ON files(session_id);
