# 22 — Server-side Config Push (CSP-style)

**Wave**: 4 | **Status**: Implemented

---

## Goal

Tune agent behaviour (retention, log levels, schedule windows) without
re-deploying the MSI via Intune.  The agent pulls a config override from the
server on startup, every 6 hours, and after every successful upload.  The
server merges a tenant-wide default with per-device overrides.

---

## Architecture

```
Operator
  │
  ├── PUT /v1/admin/config/default           (tenant-wide fallback)
  └── PUT /v1/admin/devices/{id}/config      (per-device override)
                │
                ▼
        SQLite  ──►  device_config_overrides  (per-device)
                ──►  default_config_override  (singleton)
                │
                ▼
Agent: GET /v1/config/{device_id}
        │
        ├── merge: default ◄ per-device (per-device wins)
        └── apply on top of local config
```

### Override whitelist (safe-to-push fields)

| Field                  | Type           | Description                               |
|------------------------|----------------|-------------------------------------------|
| `logLevel`             | `string`       | tracing filter directive, e.g. `"debug"`  |
| `requestTimeoutSecs`   | `u64`          | HTTP timeout (1 – 3600 s)                 |
| `evidenceSchedule`     | `string`       | Cron expression for the evidence collector|
| `queueMaxBundles`      | `usize`        | Max queued bundles on disk (1 – 10 000)   |
| `logPaths`             | `string[]`     | Replaces the full `log_paths` list        |

**Not overridable (safety boundary):** `api_endpoint`, `tls_client_cert_pem`,
`tls_client_key_pem`, `tls_ca_bundle_pem` — these could permanently disconnect
the agent if mis-configured remotely.

---

## API Routes

### `GET /v1/config/{device_id}` *(no auth required)*

Returns the merged config override for a device.

- **200 OK** — JSON body is an `AgentConfigOverride`.  Only fields that have
  been set (i.e. are non-`null`) appear in the response.
- **204 No Content** — No overrides have been configured for this device; the
  agent should use its local config unchanged.
- **500 Internal Server Error** — DB failure.

### `PUT /v1/admin/devices/{device_id}/config` *(admin role required)*

Upsert a per-device config override.  Body is a (partial) `AgentConfigOverride`
JSON object.  Missing fields are ignored (not cleared).

```json
{
  "logLevel": "debug",
  "queueMaxBundles": 10
}
```

Returns **200 OK** on success, **400 Bad Request** if a field fails validation
(e.g. `requestTimeoutSecs: 0`).

### `DELETE /v1/admin/devices/{device_id}/config` *(admin role required)*

Remove the per-device override.  Returns **204 No Content** whether a row
existed or not (idempotent).

### `PUT /v1/admin/config/default` *(admin role required)*

Upsert the tenant-wide default override.  Same body shape as the per-device
route.  Replaces the entire default (not a merge — send the complete desired
state).

---

## Safe Rollback

If the agent fails to successfully upload for **24 continuous hours** after a
remote override was applied, it automatically reverts to the last-known-good
local config (i.e. the config read from disk at startup, ignoring the override).

```
override applied at T₀
                  │
           T₀+24h ─ ──► rollback to local config if zero
                         successful uploads since T₀
```

The rollback logic lives in `crates/agent/src/config_sync.rs`:

```rust
cs.record_failure();  // called after every failed upload
if cs.should_rollback() {
    cs.rollback();
}
```

The rollback is cleared (timer reset) on the next successful upload.

---

## Agent Flow

1. **Startup** — `ConfigSync::sync()` fetches the override; `effective_config()`
   returns the merged result used to build the `Uploader` and collectors.
2. **Periodic refresh** — every `CONFIG_FETCH_INTERVAL` (6 h) the daemon calls
   `ConfigSync::sync()` again.  If the override changed, internal state updates;
   the next `Uploader`/collector operation picks up the new values.
3. **Post-upload** — after every successful drain the agent calls
   `cfg_sync.record_success()` and re-syncs to pick up any pending override
   changes without waiting for the 6-hour tick.
4. **Failure tracking** — failed uploads call `cfg_sync.record_failure()`;
   once the 24-hour threshold is crossed `should_rollback()` returns `true` and
   the daemon calls `rollback()`.

---

## Database Schema (migration 0003)

```sql
CREATE TABLE device_config_overrides (
  device_id   TEXT PRIMARY KEY,
  config_json TEXT NOT NULL,      -- JSON-encoded AgentConfigOverride
  updated_utc TEXT NOT NULL       -- ISO-8601 UTC timestamp
);

CREATE TABLE default_config_override (
  id          INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
  config_json TEXT NOT NULL,
  updated_utc TEXT NOT NULL
);
```

---

## Files Changed

| File | Change |
|------|--------|
| `crates/common-wire/src/lib.rs` | Added `AgentConfigOverride` DTO + `config` sub-module |
| `crates/api-server/migrations/0003_device_config.sql` | **NEW** — config tables |
| `crates/api-server/src/storage/mod.rs` | Added `ConfigStore` trait |
| `crates/api-server/src/storage/meta_sqlite.rs` | Implemented `ConfigStore` |
| `crates/api-server/src/routes/config.rs` | **NEW** — `GET /v1/config/{device_id}` |
| `crates/api-server/src/routes/admin.rs` | Added admin config PUT/DELETE routes |
| `crates/api-server/src/routes/mod.rs` | Registered `config` sub-module |
| `crates/api-server/src/state.rs` | Added `configs: Arc<dyn ConfigStore>` field |
| `crates/api-server/src/lib.rs` | Wired config router |
| `crates/api-server/src/main.rs` | Passed `configs` to `AppState` |
| `crates/agent/src/config_sync.rs` | **NEW** — fetch, merge, rollback |
| `crates/agent/src/lib.rs` | Registered `config_sync` module |
| `crates/agent/src/main.rs` | Wired `ConfigSync` into daemon loop |
| `crates/agent/src/tls.rs` | Added `build_reqwest_client` helper |

---

## Acceptance Criteria

- [x] Operator changes a device's override via admin route → device picks it up
      within 6 h (next `config_tick` fires `ConfigSync::sync()`).
- [x] Bad config (e.g. `requestTimeoutSecs: 0`) → server returns **400**; agent
      also validates and discards invalid server-returned payloads.
- [x] Whitelist enforced: `api_endpoint` and TLS paths are **never** remotely
      overridable (`merge_override` always copies them from the local base).
- [x] Tests cover: pull config, apply, fail-for-24h rollback trigger
      (`config_sync::tests::should_rollback_triggers_after_threshold`).

---

## Cross-references

- `crates/agent/src/config.rs` — existing local config; this PR adds the
  override layer on top.
- `docs/wave4/04-day2-operations.md` §3.B — operators want this when triaging
  "one device misbehaving".
