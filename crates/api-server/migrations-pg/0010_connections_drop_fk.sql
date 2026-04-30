-- The 0004_connections.sql migration set device_id to REFERENCE agents
-- with ON DELETE CASCADE, but the WS upgrade INSERT runs *before* the
-- first heartbeat (which is what creates the agents row), so every
-- first-time connection failed with an FK violation.
--
-- connections is operational/routing state — there's no business reason
-- for referential integrity to agents. Drop the FK; the existing
-- sweeper task (90s heartbeat timeout → mark disconnected, 5min →
-- delete) handles cleanup independently.
ALTER TABLE connections DROP CONSTRAINT IF EXISTS connections_device_id_fkey;
