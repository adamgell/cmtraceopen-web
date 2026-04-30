# Heartbeat + On-Demand Bundle Cutover Runbook

## Pre-deploy checklist

- [ ] api-server image built from the heartbeat branch and pushed to the container registry
- [ ] Pilot Postgres tier is at least B4ms (heartbeat write rate makes B2ms
      borderline at 5k+ devices under continuous heartbeat load)
- [ ] `CMTRACE_REPLICA_ID` env var is set on every api-server replica (required
      by the cross-replica forward path, T11)
- [ ] Container orchestrator ulimit is set to 65536 (ACA `resourceLimits`,
      Compose `ulimits: nofile: soft/hard: 65536`). The Dockerfile comment
      documents the value but cannot set it — the orchestrator must apply it.
- [ ] Postgres migrations 0003–0009 reviewed and a rollback migration is ready
- [ ] Agent 0.2.0 MSI is signed and uploaded to Intune + NinjaOne distribution
      points
- [ ] One canary device in the test fleet has agent 0.2.0 manually installed
      and a `wscat` smoke-test passed against the pilot environment:
      ```
      wscat -c wss://api.pilot.example.com/v1/agent/ws \
            -H "x-device-id: WIN-CANARY-01"
      # Should receive HeartbeatAck frames within 45 s
      ```

## Deploy order

1. Apply Postgres migrations 0003–0009. The api-server process will start
   ignoring WS connections until the image with the heartbeat code is deployed;
   migrations alone are non-breaking for the running 0.1.x image.

2. Deploy api-server image (heartbeat build). The `/v1/agent/ws` WebSocket
   endpoint is now live but no agents have connected yet.

3. Verify `/v1/agents` returns an empty (or stale-only) list:
   ```
   curl -s https://api.pilot.example.com/v1/agents | jq '.total'
   ```

4. Push agent 0.2.0 via Intune to a canary device tag (5–10 devices).

5. Verify connections appear in `/v1/agents` within ~2 minutes of agent restart.
   Check `last_seen_at` is recent and `agent_version = "0.2.0"`.

6. Test an operator on-demand request against the canary device:
   ```
   curl -s -X POST \
     https://api.pilot.example.com/v1/devices/WIN-CANARY-01/request-bundle \
     | jq '.request_id'
   # Poll GET /v1/devices/WIN-CANARY-01/bundle-requests/{id} until status="complete"
   ```

7. Confirm the resulting bundle appears in the web viewer with correct log content.

8. If canary holds for 30 minutes with no errors, push agent 0.2.0 to the full
   fleet via Intune + NinjaOne policy assignment.

9. Watch `/v1/agents` count climb. Legacy agents (< 0.2.0) that have not yet
   received the update will continue uploading via the existing ingest path
   (their scheduled uploads still hit `/v1/ingest/bundles`). The new heartbeat
   and on-demand paths are additive; legacy ingest is not removed.

## Rollback

If the heartbeat endpoint or the on-demand path cause stability issues:

1. Re-deploy the previous api-server image. The WS endpoint disappears; agents
   will reconnect-loop indefinitely with exponential backoff (1 s → 30 s cap)
   but will not crash or stop functioning otherwise.

2. Agent 0.2.0 with WS unreachable: the agent still drains its local queue on
   the `DRAIN_INTERVAL` (30 s) tick. If the queue is non-empty from a previous
   `--oneshot` or a previous schedule-triggered run, those bundles will still
   upload via `/v1/ingest/bundles`. New collection only happens via on-demand
   requests, which require a working WS connection. Devices go dark for new
   evidence but stay healthy.

3. To fully revert the agent, re-deploy agent 0.1.x via Intune. Agent 0.1.x
   has the local push schedule and will resume autonomous uploads.

## Known issues post-deploy

- **Legacy stragglers:** Devices running agent < 0.2.0 never connect via WS
  and are invisible to `/v1/agents`. Dashboard query for them:
  ```sql
  SELECT device_id, agent_version, last_seen_at
  FROM   agent_connections
  WHERE  last_seen_at < now() - interval '24 hours'
     OR  agent_version < '0.2.0';
  ```
  Force-update via MDM. Until updated, their bundles still arrive via the
  legacy ingest path.

- **Reconnect storm on replica restart:** If an ACA rolling update restarts all
  replicas simultaneously, all connected agents will reconnect within seconds.
  At 5k+ devices this can produce >1k concurrent `connect_and_run` calls. The
  heartbeat persister will drop heartbeats during the surge; the metric
  `cmtrace_heartbeat_drops_total` will spike. This is expected behaviour — the
  next 45 s heartbeat cycle will restore the `agent_connections` table. No
  operator action required unless the spike persists for > 2 minutes.

- **queue_depth and last_collect_at in heartbeat:** Agent 0.2.0 sends `0` for
  these fields. A follow-up commit will wire them to the runtime queue state.
  Until then, the operator dashboard should treat `queue_depth = 0` as
  "unknown" rather than "empty" for 0.2.0 agents.

## Optional feature flag

The heartbeat endpoint can be conditionally disabled at runtime with a simple
env-var check in the WS upgrade route:

```rust
if std::env::var("CMTRACE_HEARTBEAT_ENABLED").as_deref() == Ok("false") {
    return StatusCode::SERVICE_UNAVAILABLE.into_response();
}
```

This is not wired by default — the endpoint is always-enabled. Add the check
if you need a kill switch for staged rollouts. The agent's reconnect loop will
back off gracefully on a 503 response.
