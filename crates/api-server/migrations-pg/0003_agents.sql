CREATE TABLE agents (
  device_id            text PRIMARY KEY,
  device_name          text NOT NULL,
  intune_device_id     text NULL,
  ninjaone_device_id   text NULL,
  asset_tag            text NULL,
  agent_version        text NOT NULL,
  os_version           text NOT NULL,
  first_seen_at        timestamptz NOT NULL,
  last_seen_at         timestamptz NOT NULL,
  last_collect_at      timestamptz NULL,
  queue_depth          integer NOT NULL DEFAULT 0,
  errors_24h           integer NOT NULL DEFAULT 0,
  disk_free_pct        integer NOT NULL DEFAULT 100,
  uptime_seconds       bigint NOT NULL DEFAULT 0
);
CREATE INDEX agents_device_name_idx ON agents (device_name);
CREATE INDEX agents_intune_idx ON agents (intune_device_id) WHERE intune_device_id IS NOT NULL;
CREATE INDEX agents_ninjaone_idx ON agents (ninjaone_device_id) WHERE ninjaone_device_id IS NOT NULL;
CREATE INDEX agents_last_seen_idx ON agents (last_seen_at);
