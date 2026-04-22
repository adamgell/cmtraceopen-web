-- Wave 4: server-side config push.
--
-- Two tables:
--   * device_config_overrides — per-device operator-set overrides.
--   * default_config_override — singleton tenant-wide default, merged
--     before per-device overrides.
--
-- Both store the config payload as JSON (TEXT) so the schema does not need a
-- migration for every new override field — the application layer owns the
-- field whitelist (see common_wire::AgentConfigOverride).
--
-- updated_utc is an ISO-8601 TEXT timestamp kept in sync with the API write
-- time so operators can reason about staleness without a full audit log.

CREATE TABLE device_config_overrides (
  device_id   TEXT PRIMARY KEY,
  config_json TEXT NOT NULL,
  updated_utc TEXT NOT NULL
);

-- The default_config_override is a singleton row.  The CHECK constraint
-- enforces that only id=1 can ever be inserted, giving us a compile-time
-- guarantee that the table has at most one row.
CREATE TABLE default_config_override (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  config_json TEXT NOT NULL,
  updated_utc TEXT NOT NULL
);
