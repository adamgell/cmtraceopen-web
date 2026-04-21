-- ---------------------------------------------------------------------------
-- NOTE: SISTER-PR COUPLING WITH feat/parse-on-ingest
--
-- This migration defines the `files` and `entries` tables that the
-- GET /v1/sessions/{id}/files and GET /v1/sessions/{id}/entries routes in
-- this branch query against.
--
-- The authoritative schema lives in the `feat/parse-on-ingest` branch, which
-- also *populates* these tables during ingest parsing. Until that branch
-- merges, this migration serves two purposes:
--
--   1. Unblocks compile + tests in this branch (routes need real tables to
--      query against).
--   2. Documents the exact columns + indexes the query layer depends on so
--      parse-on-ingest can align its own migration to match.
--
-- If parse-on-ingest merges with a different schema, the next branch MUST
-- reconcile (either drop this migration and re-point, or evolve both).
-- Column set mirrors the FileSummary + LogEntryDto wire DTOs defined in
-- common-wire; indexes are sized to the keyset pagination queries this
-- branch uses.
-- ---------------------------------------------------------------------------

CREATE TABLE files (
    -- UUIDv7 hex string; time-sortable so keyset pagination on file_id works.
    file_id            TEXT PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES sessions(session_id),
    relative_path      TEXT NOT NULL,
    size_bytes         INTEGER NOT NULL,
    -- NULL until the parser dispatcher runs / if detection fails.
    format_detected    TEXT,
    parser_kind        TEXT,
    entry_count        INTEGER NOT NULL DEFAULT 0,
    parse_error_count  INTEGER NOT NULL DEFAULT 0
);

-- Keyset page: WHERE session_id = ? AND file_id > ? ORDER BY file_id ASC.
CREATE INDEX idx_files_session_fileid
    ON files(session_id, file_id);

CREATE TABLE entries (
    -- Autoincrementing row id; used as the tiebreaker in keyset pagination
    -- and as the stable identifier the viewer threads through when
    -- deep-linking a single entry.
    entry_id       INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id        TEXT NOT NULL REFERENCES files(file_id),
    -- Denormalized from files.session_id so the filter query doesn't need
    -- a join for the common "all entries in a session" shape.
    session_id     TEXT NOT NULL REFERENCES sessions(session_id),
    line_number    INTEGER NOT NULL,
    -- Epoch milliseconds. Nullable because some log formats (or bad lines)
    -- have no parseable timestamp.
    ts_ms          INTEGER,
    -- Numeric severity (0=info, 1=warning, 2=error). Stored numeric for
    -- cheap `severity >= ?` filters; the DTO renders it back to a string.
    severity       INTEGER NOT NULL DEFAULT 0,
    component      TEXT,
    thread         TEXT,
    message        TEXT NOT NULL,
    -- JSON blob for format-specific fields that don't have a dedicated
    -- column (http_method, result_code, ...). Surfaced to the wire as
    -- LogEntryDto::extras.
    extras_json    TEXT
);

-- Supports the main query shape:
--   WHERE session_id = ?
--     [AND file_id = ?]
--     [AND severity >= ?]
--     [AND ts_ms >= ? AND ts_ms < ?]
--   ORDER BY ts_ms NULLS LAST, entry_id
-- The composite covers the dominant session-scoped listing + keyset cursor.
CREATE INDEX idx_entries_session_ts_id
    ON entries(session_id, ts_ms, entry_id);

-- Narrower index for "all entries in a single file" (drill-down view).
CREATE INDEX idx_entries_file_ts_id
    ON entries(file_id, ts_ms, entry_id);
