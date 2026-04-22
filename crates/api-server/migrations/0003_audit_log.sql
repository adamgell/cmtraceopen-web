-- Operator / admin audit log.  Insert-only by trait contract; rows are
-- never updated or deleted.  Note that "insert-only" is enforced at the
-- application layer (the AuditStore trait + AuditSqliteStore impl), not
-- by the schema itself — a DBA or compromised process can still UPDATE
-- this table.  True tamper evidence (hash chain + verifier endpoint) is
-- tracked as a follow-up; see issue #110 and ADR 0001.
--
-- One row is written per auditable admin action (device disable/enable,
-- session reparse, bundle delete, etc.).
--
-- Column notes:
--   id             – UUID v7 (time-sortable for cheap "last N" scans).
--   ts_utc         – ISO-8601 TEXT so sqlx's chrono adapter and human
--                    inspection both work across SQLite + future Postgres.
--   principal_kind – 'operator' | 'admin' | 'device' | 'system'
--   principal_id   – JWT `sub` claim (user OID) or device cert SAN URI.
--   principal_display – `name` claim or cert CN; omit if none in token.
--   action         – dot-namespaced verb: 'device.disable', 'session.reparse',
--                    'audit.list', etc.
--   target_kind    – 'device' | 'session' | 'bundle' | NULL for meta-actions.
--   target_id      – the natural-key of the target resource; NULL when none.
--   result         – 'success' | 'failure'
--   details_json   – sanitized extras (no PII, no device hostnames).
--   request_id     – optional UUID for cross-correlating with trace logs.

CREATE TABLE audit_log (
  id                TEXT    NOT NULL PRIMARY KEY,
  ts_utc            TEXT    NOT NULL,
  principal_kind    TEXT    NOT NULL,
  principal_id      TEXT    NOT NULL,
  principal_display TEXT,
  action            TEXT    NOT NULL,
  target_kind       TEXT,
  target_id         TEXT,
  result            TEXT    NOT NULL,
  details_json      TEXT,
  request_id        TEXT
);

-- Primary access pattern: reverse-chronological page for the audit UI.
CREATE INDEX idx_audit_log_ts ON audit_log(ts_utc DESC);

-- Secondary filters operators commonly apply: "show me all actions by this
-- user" and "show me all device.disable events".
CREATE INDEX idx_audit_log_principal ON audit_log(principal_id, ts_utc DESC);
CREATE INDEX idx_audit_log_action    ON audit_log(action, ts_utc DESC);
