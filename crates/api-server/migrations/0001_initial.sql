-- Phase 3 M1 MVP schema. Timestamps stored as ISO-8601 TEXT so sqlx's
-- chrono adapter and human inspection both work. See plan doc for
-- migration path to Postgres in later milestones.

CREATE TABLE devices (
  device_id        TEXT PRIMARY KEY,
  cert_fingerprint TEXT UNIQUE,          -- nullable until mTLS lands (M2)
  first_seen_utc   TEXT NOT NULL,
  last_seen_utc    TEXT NOT NULL,
  hostname         TEXT,
  os_version       TEXT,
  enrollment_state TEXT,
  labels_json      TEXT
);

CREATE TABLE uploads (
  upload_id       TEXT PRIMARY KEY,
  bundle_id       TEXT NOT NULL,
  device_id       TEXT NOT NULL,
  size_bytes      INTEGER NOT NULL,
  expected_sha256 TEXT NOT NULL,
  content_kind    TEXT NOT NULL,
  offset_bytes    INTEGER NOT NULL DEFAULT 0,
  staged_path     TEXT NOT NULL,
  created_utc     TEXT NOT NULL,
  finalized       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_uploads_device_bundle ON uploads(device_id, bundle_id);

CREATE TABLE sessions (
  session_id     TEXT PRIMARY KEY,
  device_id      TEXT NOT NULL REFERENCES devices(device_id),
  bundle_id      TEXT NOT NULL,
  blob_uri       TEXT NOT NULL,
  content_kind   TEXT NOT NULL,
  size_bytes     INTEGER NOT NULL,
  sha256         TEXT NOT NULL,
  collected_utc  TEXT,
  ingested_utc   TEXT NOT NULL,
  parse_state    TEXT NOT NULL DEFAULT 'pending',
  UNIQUE(device_id, bundle_id)
);

CREATE INDEX idx_sessions_device_ingested ON sessions(device_id, ingested_utc DESC);
