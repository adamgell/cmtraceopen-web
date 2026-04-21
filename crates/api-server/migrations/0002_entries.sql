-- Phase 3 M2: per-session parsed entries.
--
-- When a bundle is finalized, a background parse worker walks the archive,
-- picks a parser implementation via cmtraceopen-parser's auto-detection, and
-- writes one `files` row per log file plus one `entries` row per parsed
-- LogEntry. The session's parse_state then flips from "pending" to "ok"
-- (clean), "partial" (some per-file parse errors), or "failed" (nothing
-- landed).
--
-- Severity is persisted as an int so the query path can `WHERE severity >= ?`
-- cheaply without a string-to-enum decode on every row: 0=Info, 1=Warning,
-- 2=Error. These map 1:1 from cmtraceopen_parser::models::log_entry::Severity.
--
-- ts_ms is nullable because several parsed formats (plain text, header rows,
-- etc.) may lack a timestamp. Keeping it nullable lets the viewer render those
-- entries in source order without injecting a fake time.

CREATE TABLE files (
  file_id           TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES sessions(session_id),
  relative_path     TEXT NOT NULL,
  size_bytes        INTEGER NOT NULL,
  format_detected   TEXT,
  parser_kind       TEXT,
  entry_count       INTEGER NOT NULL DEFAULT 0,
  parse_error_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE entries (
  entry_id    INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  TEXT NOT NULL,
  file_id     TEXT NOT NULL REFERENCES files(file_id),
  line_number INTEGER NOT NULL,
  ts_ms       INTEGER,            -- nullable: not every parsed entry carries a timestamp
  severity    INTEGER NOT NULL,   -- 0=Info, 1=Warning, 2=Error
  component   TEXT,
  thread      TEXT,
  message     TEXT NOT NULL,
  extras_json TEXT
);

CREATE INDEX idx_entries_session_ts ON entries(session_id, ts_ms);
CREATE INDEX idx_entries_session_severity_ts ON entries(session_id, severity, ts_ms);
CREATE INDEX idx_files_session ON files(session_id);
