CREATE TABLE heartbeats (
  id              bigserial PRIMARY KEY,
  device_id       text NOT NULL REFERENCES agents(device_id) ON DELETE CASCADE,
  ts              timestamptz NOT NULL,
  queue_depth     integer NOT NULL,
  errors_24h      integer NOT NULL,
  disk_free_pct   integer NOT NULL,
  uptime_seconds  bigint NOT NULL
);
CREATE INDEX heartbeats_device_ts_idx ON heartbeats (device_id, ts DESC);
