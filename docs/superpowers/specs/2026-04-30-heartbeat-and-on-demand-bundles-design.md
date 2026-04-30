# Heartbeat + On-Demand Bundles — Design

**Status:** approved 2026-04-30
**Owner:** Adam
**Branch target:** `feat/heartbeat-and-on-demand` (off `main` or stacked on `feat/parse-load-harness`)

## Problem

The current pilot model couples device count to ingest throughput: every agent ships on a fixed schedule (default `interval_hours = 24`), so a fleet of N devices produces ~N bundles per cadence regardless of whether anyone needs that data. At 15-minute cadence the load harness measured a hard ceiling around 5,400 devices on the existing 5-replica pilot, with Postgres write rate as the binding constraint.

The fix is to invert the relationship: agents stay connected via WebSocket and ship bundles **only when requested** — by an operator clicking a button, or by a server-side cron schedule that samples a percentage of the fleet. Device count becomes bounded by WebSocket capacity (cheap, RAM-bound, ~50–100 K idle WS per replica). Ingest throughput stays bounded by parse capacity (expensive, unchanged) but is now decoupled from fleet size.

This spec covers Phase 1 (heartbeat) + Phase 2 (on-demand bundles + cron schedules) — the self-contained ship that already changes the scaling story. Future phases (reactive triggers, summary bundles, pub/sub routing) are explicit non-goals here, with hooks left in the design for incremental addition.

## Goals

- **WebSocket-based heartbeat** — every agent maintains one long-lived WS to the api-server, sending a status frame every 45s.
- **Operator on-demand bundle** — `POST /v1/devices/{id}/request-bundle` pushes a request frame down the agent's WS; agent collects + ships via the existing ingest path.
- **Cron-style schedules** — server-side rules with `cron + selector + rate_pct + jitter + cooldown`. Deterministic-rotation sampling so coverage is uniform across days without state.
- **Hard-cutover migration** — Intune + NinjaOne push agent 0.2.0 with WS support; legacy agents (< 0.2.0) are unsupported after the rollout window.
- **Identity model that works without MDM** — `device_id` is the agent-generated PK; `device_name`, `intune_device_id`, `ninjaone_device_id`, `asset_tag` are layered metadata.

## Non-goals

- ❌ Reactive/triggered bundles based on heartbeat thresholds.
- ❌ Separate "summary bundle" schema — the heartbeat IS the lightweight observability tier.
- ❌ Cross-replica pub/sub (Redis / NATS / Service Bus). DB-pinned routing for v1.
- ❌ Selector expression language. Fixed field set with simple operators.
- ❌ Sticky load balancing — cross-replica HTTP forwarding handles routing.
- ❌ Offline-bundle queueing — operator requests to offline devices return 409 fast.
- ❌ App-level WS connection rate limits — relies on gateway-side throttling.
- ❌ Heartbeat field schema versioning — bump agent version on schema change for v1.
- ❌ Operator UI work — backend exposes endpoints; UI is a separate workstream.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                       cmtraceopen-api (server)                       │
│                                                                       │
│   Existing routes:        New routes:                                 │
│   /v1/ingest/*            /v1/agent/ws       (WebSocket upgrade)      │
│   /v1/sessions/*          /v1/devices/{id}/request-bundle             │
│                           /v1/schedules      (CRUD)                   │
│                           /v1/internal/forward  (replica → replica)   │
│                                                                       │
│   Per-replica runtime:                                                │
│   ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐  │
│   │ Connection       │  │ Heartbeat        │  │ Schedule worker  │  │
│   │ registry         │  │ persister        │  │ (one-leader)     │  │
│   │ (in-memory)      │  │ (mpsc → PG)      │  │                  │  │
│   └────────┬─────────┘  └────────▲─────────┘  └────────┬─────────┘  │
│            │                     │                     │            │
│            ▼                     │                     ▼            │
│   ┌──────────────────────────────────────────────────────────────┐  │
│   │  Postgres                                                     │  │
│   │  agents  heartbeats  schedules  connections  bundle_requests  │  │
│   └──────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
                ▲                                              │
                │ WebSocket (TLS)                              │ "request_bundle"
                │ heartbeat every 45s                          │ frame
                                  ┌──────────────────────────────┐
                                  │     cmtrace-agent (0.2.0)    │
                                  │                              │
                                  │  ws_client (long-lived)      │
                                  │  collector (existing)        │
                                  │  ingest_uploader (existing)  │
                                  └──────────────────────────────┘
```

### Components

- **`/v1/agent/ws`** — axum `WebSocketUpgrade` extractor. Auths on `X-Device-Id` (same as ingest) for v1; mTLS upgrade is a separate workstream.
- **Connection registry (per-replica, in-memory)** — `HashMap<DeviceId, mpsc::Sender<ServerFrame>>`. One mpsc per device for outbound frames; bounded buffer (e.g. 16) so a slow agent applies backpressure to dispatchers.
- **`connections` table (DB-pinned routing)** — primary key `device_id`, holds the `replica_id` that terminated this device's WS plus `connected_at` / `last_heartbeat_at`. Stale rows (5 min past `disconnected_at`) GC'd by a cleanup task.
- **Heartbeat persister** — bounded mpsc fed from each WS task; one task per replica drains and UPSERTs `agents` + appends to `heartbeats`. Drop-oldest-on-full so a slow database doesn't backpressure WS reads.
- **Schedule worker** — one elected leader per cluster (Postgres advisory lock), wakes every 30s, picks due schedules, computes the device set via deterministic rotation, fans out request frames via the connection registry or `/v1/internal/forward`. Standby replicas attempt the lock every 30s.
- **Operator endpoint** — `POST /v1/devices/{id}/request-bundle` looks up `connections.replica_id`, forwards locally or via `/v1/internal/forward`, returns 202 with `request_id`.

## Identity model

```
device_id (PK)         agent-generated at install, stable forever, opaque to server
                       Format: agent's choice (current is "<hostname-prefix>-<hex8>",
                       e.g. "GELL-2C3BD243"). Spec doesn't mandate format — just
                       "stable, unique per install, persisted in agent config, never
                       regenerated unless agent is reinstalled".

device_name            OS hostname by default; overridable via config.toml.
                       Always present (agent reads $env:COMPUTERNAME / hostname(1)).

Optional MDM IDs       (reported when present, null otherwise):
  intune_device_id     Intune managed-device ID
  ninjaone_device_id   NinjaOne agent ID
  asset_tag            Operator-set label (config.toml or registry value)
```

The system works with **just `device_id` + `device_name`**. MDM IDs are pure metadata for operator search/correlation. Adding a new MDM later = one schema migration, not a refactor.

## Wire protocol

JSON over WebSocket text frames. Schema lives in `common-wire` so agent + server share types.

### Server → Agent

```json
// "ship me a bundle now" — from operator click or scheduled fire
{
  "type": "request_bundle",
  "request_id": "<uuid>",
  "reason": "operator|scheduled",
  "schedule_name": "daily-baseline"
}

// "I see you're alive — keep that connection"
{ "type": "heartbeat_ack", "ts": "<rfc3339>" }
```

### Agent → Server

```json
// every 45s (also acts as "hello" when first sent after WS connect)
{
  "type": "heartbeat",
  "device_id": "GELL-2C3BD243",
  "device_name": "GELL-LAPTOP-01",
  "intune_device_id": null,
  "ninjaone_device_id": null,
  "asset_tag": null,
  "ts": "<rfc3339>",
  "agent_version": "0.2.0",
  "os_version": "Windows 10.0.22631",
  "last_collect_at": "<rfc3339|null>",
  "queue_depth": 0,
  "errors_24h": 42,
  "disk_free_pct": 71,
  "uptime_seconds": 189342
}

// after a request_bundle arrives — agent acks before doing work
{
  "type": "request_ack",
  "device_id": "GELL-2C3BD243",
  "device_name": "GELL-LAPTOP-01",
  "request_id": "<uuid>",
  "accepted": true
}

// after the bundle is shipped via existing /v1/ingest path
{
  "type": "request_complete",
  "device_id": "GELL-2C3BD243",
  "device_name": "GELL-LAPTOP-01",
  "request_id": "<uuid>",
  "bundle_id": "<uuid>",
  "outcome": "ok|error",
  "error": "<string|null>"
}
```

### Connection lifecycle

1. Agent opens `wss://{api}/v1/agent/ws` with `X-Device-Id` header.
2. Server accepts, UPSERTs `connections(device_id, replica_id, ...)`.
3. Agent immediately sends a `heartbeat` (acts as "hello").
4. Loop: every 45s send `heartbeat`, expect `heartbeat_ack` within 30s. If 2 consecutive ACKs missed → close + reconnect with backoff (1s → 2s → 5s → 15s → 30s, capped + jittered).
5. When `request_bundle` arrives: agent sends `request_ack` immediately, runs collection, calls existing `/v1/ingest/bundles` with `X-Bundle-Request-Id` header so the server can correlate the resulting session with the originating request.
6. On graceful shutdown (server SIGTERM): server sends WS close 1001 (going away); agent reconnects to whichever replica picks it up next.

### Server-side timeouts

- No heartbeat for 90s (2 missed cycles + grace) → close WS, set `connections.disconnected_at = now()`. Row GC'd 5 min later.
- Idle ping/pong every 30s (WS sub-protocol) keeps NAT/proxy state warm in addition to the app-level heartbeat.

### Cross-replica forwarding

`POST /v1/internal/forward` — internal-only, mTLS between replicas (or `CMTRACE_INTER_REPLICA_TOKEN` shared secret for v1).

```json
{ "device_id": "...", "frame": { "type": "request_bundle", ... } }
```

Receiving replica looks up its in-memory registry and drops the frame onto the device's mpsc. Returns 202 immediately; actual WS send is fire-and-forget.

### Header-bound auth check

The `device_id` field in every frame is verified to match the `X-Device-Id` header used at WS handshake. Mismatch → WS close 1008 (policy violation), security log event. Belt-and-suspenders against future auth bugs.

## Data model

```sql
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
  queue_depth          int NOT NULL DEFAULT 0,
  errors_24h           int NOT NULL DEFAULT 0,
  disk_free_pct        int NOT NULL DEFAULT 100,
  uptime_seconds       bigint NOT NULL DEFAULT 0
);
CREATE INDEX agents_device_name_idx ON agents (device_name);
CREATE INDEX agents_intune_idx ON agents (intune_device_id) WHERE intune_device_id IS NOT NULL;
CREATE INDEX agents_ninjaone_idx ON agents (ninjaone_device_id) WHERE ninjaone_device_id IS NOT NULL;
CREATE INDEX agents_last_seen_idx ON agents (last_seen_at);

CREATE TABLE connections (
  device_id          text PRIMARY KEY REFERENCES agents(device_id),
  replica_id         text NOT NULL,            -- env CMTRACE_REPLICA_ID
  connected_at       timestamptz NOT NULL,
  last_heartbeat_at  timestamptz NOT NULL,
  disconnected_at    timestamptz NULL          -- non-null = stale, gc'd after 5 min
);
CREATE INDEX connections_replica_idx ON connections (replica_id) WHERE disconnected_at IS NULL;

CREATE TABLE heartbeats (
  id             bigserial PRIMARY KEY,
  device_id      text NOT NULL REFERENCES agents(device_id),
  ts             timestamptz NOT NULL,
  queue_depth    int NOT NULL,
  errors_24h     int NOT NULL,
  disk_free_pct  int NOT NULL,
  uptime_seconds bigint NOT NULL
);
CREATE INDEX heartbeats_device_ts_idx ON heartbeats (device_id, ts DESC);
-- Daily worker truncates rows older than 24h.

CREATE TABLE schedules (
  name              text PRIMARY KEY,
  cron              text NOT NULL,
  selector_json     jsonb NOT NULL,
  rate_pct          int NOT NULL CHECK (rate_pct BETWEEN 1 AND 100),
  jitter_seconds    int NOT NULL DEFAULT 0,
  cooldown_seconds  int NOT NULL DEFAULT 0,
  enabled           boolean NOT NULL DEFAULT true,
  last_fired_at     timestamptz NULL,
  next_fire_at      timestamptz NOT NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX schedules_due_idx ON schedules (next_fire_at) WHERE enabled = true;

-- `request_id` is generated server-side (UUID v4) at row insertion: the
-- operator POST returns it in the 202 body; the schedule worker generates
-- it before dispatching the frame.
CREATE TABLE bundle_requests (
  request_id      uuid PRIMARY KEY,
  device_id       text NOT NULL REFERENCES agents(device_id),
  source          text NOT NULL CHECK (source IN ('operator', 'scheduled')),
  schedule_name   text NULL REFERENCES schedules(name) ON DELETE SET NULL,
  operator_email  text NULL,
  requested_at    timestamptz NOT NULL,
  acked_at        timestamptz NULL,
  completed_at    timestamptz NULL,
  bundle_id       uuid NULL,
  outcome         text NULL CHECK (outcome IN ('ok','error','timeout','offline')),
  error           text NULL
);
CREATE INDEX bundle_requests_device_idx ON bundle_requests (device_id, requested_at DESC);
CREATE INDEX bundle_requests_schedule_idx ON bundle_requests (schedule_name, requested_at DESC);
```

## Schedule engine

### Selector grammar (v1, fixed fields)

The `selector_json` blob is validated on `POST /v1/schedules`. Allowed fields:

- `device_id` — exact match or list (`["a", "b"]`)
- `device_name` — exact / prefix (`"prefix:LAB-"`) / glob
- `intune_device_id` — exact / list / `"is_set"` / `"is_null"`
- `ninjaone_device_id` — exact / list / `"is_set"` / `"is_null"`
- `asset_tag` — exact / prefix / list
- `os_version` — prefix
- `agent_version` — exact / range (`">=0.2.0"`)
- `last_seen_within` — duration string (`"1h"`, `"15m"`, `"30s"`)

Multiple fields combine with logical AND. Lists within a single field combine with OR.

### Worker loop (one leader per cluster)

```rust
loop {
    let _lock = pg_try_advisory_lock(SCHEDULE_LEADER_KEY).await?;  // standby retries every 30s

    loop {
        let due: Vec<Schedule> = sqlx::query_as(
            "SELECT * FROM schedules
             WHERE enabled AND next_fire_at <= now()
             ORDER BY next_fire_at ASC LIMIT 16"
        ).fetch_all(&pool).await?;

        for s in due {
            fire_schedule(&pool, &s).await;
            let next = compute_next_fire(&s.cron, now);
            sqlx::query("UPDATE schedules SET last_fired_at = now(), next_fire_at = $1 WHERE name = $2")
                .bind(next).bind(&s.name).execute(&pool).await?;
        }

        sleep(30s).await;
    }
}
```

### `fire_schedule(s)` — deterministic rotation

1. Resolve selector → list of matching `device_id`s from `agents`.
2. Total = matched count, K = `ceil(total * rate_pct / 100)`.
3. Compute today's UTC epoch-day as the rotation salt.
4. For each device d, compute `hash = sha256(d.device_id || epoch_day)` as a u64.
5. Take the K devices with the lowest hash values (sort ascending, take top K).
6. Filter out any device with a `bundle_requests` row newer than `(now - cooldown_seconds)`.
7. For each remaining device:
   - Insert `bundle_requests` row with `source='scheduled'`, `schedule_name=s.name`.
   - Compute `jitter_delay = random(0, jitter_seconds)`.
   - Spawn a task that sleeps `jitter_delay` then dispatches the `request_bundle` frame via the connection registry or `/v1/internal/forward`.

**Properties:**
- Deterministic same-day: re-running picks the same K devices (epoch-day salt is stable). Idempotent.
- Different across days: epoch-day rotates the hash → tomorrow's K is mostly different.
- No state to maintain — the hash is the rotation primitive.
- Fairness: sha256 over device_id space gives uniform-ish picks.

### Cron handling

Use the `cron` Rust crate. Standard 5-field syntax (`min hour dom mon dow`). Validated at INSERT time; invalid expressions return 400 from `POST /v1/schedules`. `compute_next_fire` is `cron::Schedule::upcoming(Utc).next()`.

## Operator request path

```
POST /v1/devices/{device_id}/request-bundle
  body: { "operator_email": "<from auth>", "reason": "<free-text>" }
```

1. Validate the device exists in `agents`.
2. Insert `bundle_requests` row (`source='operator'`, `schedule_name=NULL`).
3. Look up `connections.replica_id WHERE device_id=? AND disconnected_at IS NULL`.
   - Null → 409 `"device offline"`. Request row stays with `outcome=NULL` (operator may retry when device returns).
   - Self → push to local connection registry's mpsc.
   - Other replica → POST `/v1/internal/forward` to that replica.
4. Return 202 `{ "request_id": "...", "device": "...", "status": "dispatched" }`.

The viewer's Device page polls `GET /v1/devices/{id}/bundle-requests?limit=50` to show request history (operator + scheduled mixed) with outcome and bundle link.

## Migration / cutover

Hard cutover via Intune + NinjaOne; no legacy compatibility window.

1. **Server side:** ship server with WS endpoint behind `CMTRACE_HEARTBEAT_ENABLED` feature flag. Default off. Pilot redeploy turns it on.
2. **Agent side:** ship agent 0.2.0 that connects to WS on startup, sends heartbeats, handles `request_bundle`. Drops the per-agent shipping schedule (becomes pull-only).
3. **Rollout:** Intune + NinjaOne push 0.2.0 to fleet. Operator monitors `/v1/agents` for `last_seen_at` rising as agents reconnect.
4. **Old agents:** any agent < 0.2.0 stops shipping bundles after its existing schedule is removed locally; goes silent. Server marks `last_seen_at < now() - 24h` and a dashboard surfaces "stragglers needing update". MDM version-pin forces upgrade.
5. **Deprecate `CMTRACE_SCHEDULE_INTERVAL_HOURS`** — agent still parses it but logs deprecation; ignored once WS connects successfully.

## Error handling

| Failure | Behavior |
|---|---|
| Server can't reach Postgres on startup | Process crashes (existing behavior) — k8s/ACA restarts. |
| Agent WS connect fails (TLS, 401, network) | Reconnect with exponential backoff: 1s → 2s → 5s → 15s → 30s, capped + jittered. Logged locally only. |
| Agent disconnects mid-bundle-request | `bundle_requests.outcome = 'timeout'` after 10 min if no `request_complete`. Cooldown still applies — no retry storm on the device. |
| Heartbeat persister mpsc full | Drop oldest heartbeats; metric `cmtrace_heartbeat_drops_total` counts losses. |
| Schedule worker leader dies | Advisory lock auto-releases; another replica grabs it within ~30s. In-flight `bundle_requests` rows survive in DB; duplicate dispatch protected by cooldown + unique `request_id`. |
| Cross-replica forward POST fails | Schedule worker logs + marks `bundle_requests.outcome = 'offline'`. Cooldown still applies. |
| Agent receives malformed `request_bundle` frame | WS close 1003 (unsupported data), reconnect. Server logs the event. |
| Device sends `device_id` mismatching its WS auth header | WS close 1008 (policy violation), security log event, no reconnect grace. |
| Schedule worker advisory lock contention | Replica that loses the race becomes a standby; no work lost. |

## Testing

### Unit tests (in-process, no real WS)

- Frame ser/de round-trip (every message type, including unknown-field tolerance).
- `compute_next_fire` is correct against fixed test cases (cron expressions are interpreted in UTC; no DST handling needed).
- Deterministic-rotation: given a fixed device set and rate%, picks the same K devices on the same epoch-day; picks different (but uniform-ish) K on the next day; uniformity check via chi-square over many days.
- Selector resolution: SQL generation from JSON selector for every supported field + every operator.
- Cooldown enforcement: two schedules with overlapping selectors don't double-pick within the cooldown window.

### Integration tests (axum test server + sqlx PG fixture)

- WS connect → send heartbeat → assert UPSERT into `agents`.
- Send `request_bundle` from operator endpoint → verify it lands on the WS receiver task → verify `bundle_requests` row.
- Schedule fire end-to-end: insert schedule, insert N agents, set `next_fire_at = now()`, run worker tick, assert `N×rate_pct%` rows in `bundle_requests` with correct `schedule_name`.
- Cross-replica forwarding: spin up two axum instances with different `replica_id`s, connect a WS to one, dispatch from the other, verify forwarded frame arrives.
- Reconnect: kill the agent's WS, wait 90s, verify `connections.disconnected_at` set; reconnect, verify a new row replaces the stale one.
- `device_id`/header mismatch → WS close 1008.

### Load tests (parse-load-harness)

- New `--target=local-http-ws` shape opens WS connections alongside ingest. Heartbeat-only stress: 10K concurrent WS, verify RAM stays bounded and heartbeat persister keeps up.
- Schedule-fire stress: insert 50K agents into the test DB, fire a 10% rate schedule, measure dispatch latency distribution + cooldown correctness.

## Hooks for future phases (deferred)

Each is intentionally easy to add on top of v1:

1. **Reactive triggers** — add a worker that watches `heartbeats` for thresholds and inserts `bundle_requests` rows.
2. **Summary bundles** — new agent collection mode + new `content_kind` value in ingest.
3. **Pub/sub routing** — swap the `/v1/internal/forward` HTTP call with NATS / Redis / Service Bus publish.
4. **Sticky LB / mTLS WS auth / heartbeat compression** — listed in non-goals; revisit when measured load demands them.

## Open questions deferred

- **Heartbeat retention beyond 24h** — if operator UI wants weekly trends, expand to 7d + a daily aggregate roll-up.
- **`X-Device-Id` → mTLS** — same trust model as ingest. mTLS upgrade is a separate workstream affecting both ingest and WS.
- **Operator UI scope** — backend exposes endpoints; the frontend (`viewer`) work is a separate ticket.
- **NinjaOne vs Intune as canonical MDM ID source** — both columns coexist; whichever is set is queryable. Operator decides which to use per-fleet.
