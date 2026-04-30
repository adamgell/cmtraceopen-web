CREATE TABLE connections (
  device_id          text PRIMARY KEY REFERENCES agents(device_id) ON DELETE CASCADE,
  replica_id         text NOT NULL,
  connected_at       timestamptz NOT NULL,
  last_heartbeat_at  timestamptz NOT NULL,
  disconnected_at    timestamptz NULL
);
CREATE INDEX connections_replica_idx ON connections (replica_id) WHERE disconnected_at IS NULL;
