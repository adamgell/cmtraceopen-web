CREATE TABLE bundle_requests (
  request_id        uuid PRIMARY KEY,
  device_id         text NOT NULL REFERENCES agents(device_id) ON DELETE CASCADE,
  source            text NOT NULL CHECK (source IN ('operator', 'scheduled')),
  schedule_name     text NULL REFERENCES schedules(name) ON DELETE SET NULL,
  operator_email    text NULL,
  requested_at      timestamptz NOT NULL,
  acked_at          timestamptz NULL,
  completed_at      timestamptz NULL,
  bundle_id         uuid NULL,
  outcome           text NULL CHECK (outcome IN ('ok','error','timeout','offline')),
  error             text NULL,
  forward_attempts  smallint NOT NULL DEFAULT 0
);
CREATE INDEX bundle_requests_device_idx ON bundle_requests (device_id, requested_at DESC);
CREATE INDEX bundle_requests_schedule_idx ON bundle_requests (schedule_name, requested_at DESC);
