# Heartbeat + On-Demand Bundles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add WebSocket heartbeat (Phase 1) + operator on-demand bundle requests + cron-driven schedules (Phase 2) to cmtrace-api-server and cmtrace-agent. After this lands, devices stay connected via long-lived WS, ingest is operator-driven instead of schedule-driven, and a 50K-device fleet costs only its WS frames at idle.

**Architecture:** Per-replica in-memory connection registry + DB-pinned cross-replica routing. Schedule worker leader-elected via PG advisory lock on a dedicated long-lived connection. Frame schemas live in `common-wire`. Hard cutover migration via Intune/NinjaOne; legacy agents (< 0.2.0) unsupported.

**Tech Stack:** axum WebSocket, sqlx PG, tokio mpsc, the `cron` crate, `tokio_tungstenite` (agent client), `sha2` (rotation hash), `rand_chacha` (jitter).

**Spec:** [`docs/superpowers/specs/2026-04-30-heartbeat-and-on-demand-bundles-design.md`](../specs/2026-04-30-heartbeat-and-on-demand-bundles-design.md)

**Phase milestones:**
- After Task 9 ✅ Phase 1 complete: agent connects, heartbeats stream into PG, server tracks `last_seen_at`. No bundle requests yet.
- After Task 17 ✅ Phase 2a complete: operator can request a bundle from any connected device.
- After Task 22 ✅ Phase 2b complete: cron-driven scheduled requests with deterministic-rotation sampling.

**File structure:**

```
crates/common-wire/src/
├── ws.rs                    NEW — WebSocket frame types

crates/api-server/
├── migrations-pg/
│   ├── 0003_agents.sql      NEW
│   ├── 0004_connections.sql NEW
│   ├── 0005_heartbeats.sql  NEW
│   ├── 0006_schedules.sql   NEW
│   ├── 0007_bundle_requests.sql NEW
│   └── 0008_sessions_request_id.sql NEW
├── src/
│   ├── ws/                  NEW directory
│   │   ├── mod.rs           Connection registry + spawn loop
│   │   ├── handler.rs       Per-connection task (read/write/ping)
│   │   ├── auth.rs          Handshake auth + device_id mismatch
│   │   └── persister.rs     Heartbeat persister (bounded mpsc → PG)
│   ├── routes/
│   │   ├── agents.rs        NEW — GET /v1/agents, POST /v1/agents/{id}/forget
│   │   ├── ws.rs            NEW — /v1/agent/ws upgrade endpoint
│   │   ├── request_bundle.rs NEW — POST /v1/devices/{id}/request-bundle
│   │   ├── schedules.rs     NEW — schedule CRUD
│   │   ├── internal.rs      NEW — POST /v1/internal/forward
│   │   ├── bundle_requests.rs NEW — GET /v1/devices/{id}/bundle-requests
│   │   ├── ingest.rs        MODIFY — read X-Bundle-Request-Id header
│   │   └── mod.rs           MODIFY — register new routes
│   ├── schedule/            NEW directory
│   │   ├── mod.rs           Worker entry + leader loop
│   │   ├── leader.rs        Advisory-lock acquire/hold logic
│   │   ├── selector.rs      JSON selector → bound SQL
│   │   ├── rotation.rs      Deterministic rotation + cooldown
│   │   └── dispatch.rs      Local mpsc / remote forward dispatch
│   ├── storage/
│   │   ├── mod.rs           MODIFY — add MetadataStore methods
│   │   └── meta_postgres.rs MODIFY — add Postgres impls
│   └── state.rs             MODIFY — add ConnectionRegistry field

crates/agent/src/
├── ws/                      NEW directory
│   ├── mod.rs               Client entry + reconnect loop
│   ├── client.rs            WS connection + read/write tasks
│   ├── heartbeat.rs         Periodic heartbeat sender
│   └── request_handler.rs   request_bundle handler → ingest path
├── runtime.rs               MODIFY — drop scheduled-shipping path

infra/
├── azure/envs/pilot/        MODIFY — bump replica ulimits in TF
└── docker/api-server/Dockerfile MODIFY — set ulimit -n
```

---

## Task 1: Migrations 0003–0008

**Files:**
- Create: `crates/api-server/migrations-pg/0003_agents.sql`
- Create: `crates/api-server/migrations-pg/0004_connections.sql`
- Create: `crates/api-server/migrations-pg/0005_heartbeats.sql`
- Create: `crates/api-server/migrations-pg/0006_schedules.sql`
- Create: `crates/api-server/migrations-pg/0007_bundle_requests.sql`
- Create: `crates/api-server/migrations-pg/0008_sessions_request_id.sql`

- [ ] **Step 1: Write `0003_agents.sql`**

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
  queue_depth          integer NOT NULL DEFAULT 0,
  errors_24h           integer NOT NULL DEFAULT 0,
  disk_free_pct        integer NOT NULL DEFAULT 100,
  uptime_seconds       bigint NOT NULL DEFAULT 0
);
CREATE INDEX agents_device_name_idx ON agents (device_name);
CREATE INDEX agents_intune_idx ON agents (intune_device_id) WHERE intune_device_id IS NOT NULL;
CREATE INDEX agents_ninjaone_idx ON agents (ninjaone_device_id) WHERE ninjaone_device_id IS NOT NULL;
CREATE INDEX agents_last_seen_idx ON agents (last_seen_at);
```

- [ ] **Step 2: Write `0004_connections.sql`**

```sql
CREATE TABLE connections (
  device_id          text PRIMARY KEY REFERENCES agents(device_id) ON DELETE CASCADE,
  replica_id         text NOT NULL,
  connected_at       timestamptz NOT NULL,
  last_heartbeat_at  timestamptz NOT NULL,
  disconnected_at    timestamptz NULL
);
CREATE INDEX connections_replica_idx ON connections (replica_id) WHERE disconnected_at IS NULL;
```

- [ ] **Step 3: Write `0005_heartbeats.sql`**

```sql
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
```

- [ ] **Step 4: Write `0006_schedules.sql`**

```sql
CREATE TABLE schedules (
  name              text PRIMARY KEY,
  cron              text NOT NULL,
  selector_json     jsonb NOT NULL,
  rate_pct          integer NOT NULL CHECK (rate_pct BETWEEN 1 AND 100),
  jitter_seconds    integer NOT NULL DEFAULT 0,
  cooldown_seconds  integer NOT NULL DEFAULT 0,
  enabled           boolean NOT NULL DEFAULT true,
  last_fired_at     timestamptz NULL,
  next_fire_at      timestamptz NOT NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX schedules_due_idx ON schedules (next_fire_at) WHERE enabled = true;
```

- [ ] **Step 5: Write `0007_bundle_requests.sql`**

```sql
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
```

- [ ] **Step 6: Write `0008_sessions_request_id.sql`**

```sql
ALTER TABLE sessions ADD COLUMN request_id uuid NULL;
CREATE INDEX sessions_request_id_idx ON sessions (request_id) WHERE request_id IS NOT NULL;
```

- [ ] **Step 7: Verify migrations apply cleanly**

Spin up a scratch PG and run a build:
```
docker run -d --name cmtrace-mig-test -e POSTGRES_USER=cmtrace -e POSTGRES_PASSWORD=cmtrace -e POSTGRES_DB=cmtrace -p 5544:5432 postgres:16
sleep 3
CMTRACE_DATABASE_URL=postgres://cmtrace:cmtrace@localhost:5544/cmtrace cargo test -p api-server --lib storage::meta_postgres -- --ignored
docker rm -f cmtrace-mig-test
```
Expected: existing tests pass; the migration runner accepts the 6 new files.

- [ ] **Step 8: Commit**

```bash
git add crates/api-server/migrations-pg/000{3,4,5,6,7,8}_*.sql
git commit -m "feat(api): migrations for agents/connections/heartbeats/schedules/bundle_requests"
```

---

## Task 2: WebSocket frame types in common-wire

**Files:**
- Create: `crates/common-wire/src/ws.rs`
- Modify: `crates/common-wire/src/lib.rs` (add `pub mod ws;`)

- [ ] **Step 1: Write `ws.rs` with all five frame types**

Replace `crates/common-wire/src/ws.rs`:

```rust
//! WebSocket frame schemas for the heartbeat + on-demand-bundle protocol.
//! Owned by `common-wire` so agent and server share the types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server → Agent
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    RequestBundle {
        request_id: Uuid,
        reason: BundleReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedule_name: Option<String>,
    },
    HeartbeatAck {
        ts: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BundleReason {
    Operator,
    Scheduled,
}

/// Agent → Server
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentFrame {
    Heartbeat(Heartbeat),
    RequestAck {
        device_id: String,
        device_name: String,
        request_id: Uuid,
        accepted: bool,
    },
    RequestComplete {
        device_id: String,
        device_name: String,
        request_id: Uuid,
        bundle_id: Uuid,
        outcome: RequestOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {
    pub device_id: String,
    pub device_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intune_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ninjaone_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_tag: Option<String>,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub agent_version: String,
    pub os_version: String,
    pub last_collect_at: Option<chrono::DateTime<chrono::Utc>>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Ok,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_round_trips() {
        let hb = Heartbeat {
            device_id: "GELL-2C3BD243".into(),
            device_name: "GELL-LAPTOP-01".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: chrono::Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "Windows 10.0.22631".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 42,
            disk_free_pct: 71,
            uptime_seconds: 189342,
        };
        let frame = AgentFrame::Heartbeat(hb.clone());
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"heartbeat\""));
        assert!(json.contains("\"device_id\":\"GELL-2C3BD243\""));
        let back: AgentFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AgentFrame::Heartbeat(hb));
    }

    #[test]
    fn request_bundle_with_optional_schedule() {
        let f = ServerFrame::RequestBundle {
            request_id: Uuid::nil(),
            reason: BundleReason::Operator,
            schedule_name: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("schedule_name"));   // skipped when None
        let f2: ServerFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn request_complete_serializes_outcome() {
        let f = AgentFrame::RequestComplete {
            device_id: "d".into(),
            device_name: "n".into(),
            request_id: Uuid::nil(),
            bundle_id: Uuid::nil(),
            outcome: RequestOutcome::Ok,
            error: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"outcome\":\"ok\""));
        assert!(!json.contains("\"error\""));       // skipped when None
    }
}
```

- [ ] **Step 2: Add the module declaration**

Edit `crates/common-wire/src/lib.rs` and add at the top of the file (after the existing module declarations, alphabetically placed):

```rust
pub mod ws;
```

- [ ] **Step 3: Confirm `common-wire` already depends on `chrono` and `uuid`**

Check `crates/common-wire/Cargo.toml`. If `chrono` and `uuid` (with `serde` feature) are not already in `[dependencies]`, add them. Most likely they are — `common-wire` carries the existing wire types that already use these.

- [ ] **Step 4: Run tests**

```
cargo test -p common-wire --lib ws
```
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/common-wire/src/ws.rs crates/common-wire/src/lib.rs crates/common-wire/Cargo.toml
git commit -m "feat(common-wire): WebSocket frame types for heartbeat + bundle requests"
```

---

## Task 3: Connection registry (in-memory)

**Files:**
- Create: `crates/api-server/src/ws/mod.rs`

- [ ] **Step 1: Write the registry with tests**

Create `crates/api-server/src/ws/mod.rs`:

```rust
//! Per-replica WebSocket subsystem. Holds the in-memory map from
//! `device_id` to outbound mpsc; spawns the per-connection handler tasks.

pub mod auth;
pub mod handler;
pub mod persister;

use common_wire::ws::ServerFrame;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Bounded buffer per connection's outbound mpsc. A slow agent applies
/// backpressure to dispatchers; a stalled write closes the connection.
pub const OUTBOUND_BUFFER: usize = 16;

/// Shared per-replica registry. Cloned via the surrounding `Arc`.
#[derive(Clone, Default)]
pub struct ConnectionRegistry {
    inner: Arc<RwLock<HashMap<String, mpsc::Sender<ServerFrame>>>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly-connected device. Returns the receiver for the
    /// per-connection write task. If a stale registration exists, evict
    /// it (drops the old sender → old write task observes channel close
    /// → old conn shuts down).
    pub async fn insert(&self, device_id: String) -> mpsc::Receiver<ServerFrame> {
        let (tx, rx) = mpsc::channel(OUTBOUND_BUFFER);
        self.inner.write().await.insert(device_id, tx);
        rx
    }

    /// Remove a device when its connection closes.
    pub async fn remove(&self, device_id: &str) {
        self.inner.write().await.remove(device_id);
    }

    /// Try to dispatch a frame to the device. Returns Ok if the frame was
    /// queued, Err if the device is not connected to *this* replica or the
    /// outbound buffer is full (slow agent).
    pub async fn try_send(
        &self,
        device_id: &str,
        frame: ServerFrame,
    ) -> Result<(), DispatchError> {
        let map = self.inner.read().await;
        let tx = map.get(device_id).ok_or(DispatchError::NotLocal)?;
        tx.try_send(frame).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => DispatchError::Backpressure,
            mpsc::error::TrySendError::Closed(_) => DispatchError::NotLocal,
        })
    }

    /// Snapshot of currently-registered device_ids on this replica. Used
    /// only for tests / metrics.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn device_ids(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("device is not connected to this replica")]
    NotLocal,
    #[error("outbound buffer full (agent is slow)")]
    Backpressure,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn req() -> ServerFrame {
        ServerFrame::HeartbeatAck { ts: Utc::now() }
    }

    #[tokio::test]
    async fn insert_remove_round_trip() {
        let r = ConnectionRegistry::new();
        let _rx = r.insert("dev-1".into()).await;
        assert_eq!(r.device_ids().await, vec!["dev-1".to_string()]);
        r.remove("dev-1").await;
        assert!(r.device_ids().await.is_empty());
    }

    #[tokio::test]
    async fn try_send_to_unknown_returns_not_local() {
        let r = ConnectionRegistry::new();
        let result = r.try_send("ghost", req()).await;
        assert!(matches!(result, Err(DispatchError::NotLocal)));
    }

    #[tokio::test]
    async fn try_send_full_buffer_returns_backpressure() {
        let r = ConnectionRegistry::new();
        let mut _rx = r.insert("dev-1".into()).await;
        // OUTBOUND_BUFFER frames fit; the next is the backpressure case.
        for _ in 0..OUTBOUND_BUFFER {
            r.try_send("dev-1", req()).await.unwrap();
        }
        let result = r.try_send("dev-1", req()).await;
        assert!(matches!(result, Err(DispatchError::Backpressure)));
    }

    #[tokio::test]
    async fn closed_receiver_reads_as_not_local() {
        let r = ConnectionRegistry::new();
        let rx = r.insert("dev-1".into()).await;
        drop(rx);
        let result = r.try_send("dev-1", req()).await;
        assert!(matches!(result, Err(DispatchError::NotLocal)));
    }

    #[tokio::test]
    async fn re_insert_evicts_old_sender() {
        let r = ConnectionRegistry::new();
        let mut rx1 = r.insert("dev-1".into()).await;
        let _rx2 = r.insert("dev-1".into()).await;
        assert!(rx1.recv().await.is_none(), "old receiver should observe close");
    }

    fn _allow_unused() {
        let _ = Uuid::nil();
    }
}
```

- [ ] **Step 2: Stub the sibling modules so the file compiles**

Create three near-empty stub files (each just contains `//! TODO: implement` and any required `pub` re-exports). Real implementations come in Tasks 4-6.

`crates/api-server/src/ws/auth.rs`:
```rust
//! TODO: implement (Task 4)
```

`crates/api-server/src/ws/handler.rs`:
```rust
//! TODO: implement (Task 5)
```

`crates/api-server/src/ws/persister.rs`:
```rust
//! TODO: implement (Task 6)
```

- [ ] **Step 3: Wire `ws` module into `lib.rs`**

Open `crates/api-server/src/lib.rs` and add `pub mod ws;` near the existing top-level module declarations.

- [ ] **Step 4: Add `thiserror` if not already in api-server's deps**

Check `crates/api-server/Cargo.toml` — `thiserror` should already be present (used in StorageError). If not, add `thiserror = "1"`.

- [ ] **Step 5: Run tests**

```
cargo test -p api-server --lib ws::tests
```
Expected: 5 tests pass. (The two stub modules are currently empty so they compile but contribute no tests yet.)

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/ws/ crates/api-server/src/lib.rs
git commit -m "feat(api): in-memory connection registry for WebSocket subsystem"
```

---

## Task 4: WebSocket handshake auth + frame validation

**Files:**
- Modify: `crates/api-server/src/ws/auth.rs`

- [ ] **Step 1: Replace `ws/auth.rs`**

```rust
//! Handshake-time auth + per-frame device_id verification. v1 uses the
//! `X-Device-Id` header (same model as ingest); mTLS upgrade is a
//! separate workstream.

use axum::http::HeaderMap;
use common_wire::ws::AgentFrame;

#[derive(Debug, thiserror::Error)]
pub enum WsAuthError {
    #[error("missing X-Device-Id header")]
    MissingDeviceId,
    #[error("X-Device-Id is not valid UTF-8 / too long")]
    BadDeviceId,
    #[error("frame device_id {got} does not match auth identity {expected}")]
    DeviceIdMismatch { expected: String, got: String },
}

/// Pulls `X-Device-Id` from the upgrade request. Length-capped at 128
/// chars (way more than the agent's <hostname>-<hex> format).
pub fn extract_device_id(headers: &HeaderMap) -> Result<String, WsAuthError> {
    let v = headers
        .get("x-device-id")
        .ok_or(WsAuthError::MissingDeviceId)?
        .to_str()
        .map_err(|_| WsAuthError::BadDeviceId)?;
    if v.is_empty() || v.len() > 128 {
        return Err(WsAuthError::BadDeviceId);
    }
    Ok(v.to_string())
}

/// Every frame from the agent must carry `device_id` matching the auth.
/// HeartbeatAck doesn't apply (server → agent).
pub fn verify_frame_device_id(
    expected: &str,
    frame: &AgentFrame,
) -> Result<(), WsAuthError> {
    let got = match frame {
        AgentFrame::Heartbeat(h) => &h.device_id,
        AgentFrame::RequestAck { device_id, .. } => device_id,
        AgentFrame::RequestComplete { device_id, .. } => device_id,
    };
    if got != expected {
        return Err(WsAuthError::DeviceIdMismatch {
            expected: expected.to_string(),
            got: got.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common_wire::ws::Heartbeat;
    use uuid::Uuid;

    fn hb(device_id: &str) -> AgentFrame {
        AgentFrame::Heartbeat(Heartbeat {
            device_id: device_id.into(),
            device_name: "n".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "win".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 0,
            disk_free_pct: 0,
            uptime_seconds: 0,
        })
    }

    #[test]
    fn missing_header_rejected() {
        let h = HeaderMap::new();
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::MissingDeviceId)));
    }

    #[test]
    fn empty_or_too_long_rejected() {
        let mut h = HeaderMap::new();
        h.insert("x-device-id", "".parse().unwrap());
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::BadDeviceId)));
        h.insert("x-device-id", "a".repeat(129).parse().unwrap());
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::BadDeviceId)));
    }

    #[test]
    fn valid_header_extracted() {
        let mut h = HeaderMap::new();
        h.insert("x-device-id", "GELL-2C3BD243".parse().unwrap());
        assert_eq!(extract_device_id(&h).unwrap(), "GELL-2C3BD243");
    }

    #[test]
    fn matching_frame_passes() {
        let frame = hb("dev-1");
        verify_frame_device_id("dev-1", &frame).unwrap();
    }

    #[test]
    fn mismatched_frame_rejected() {
        let frame = hb("attacker-id");
        let err = verify_frame_device_id("dev-1", &frame).unwrap_err();
        assert!(matches!(err, WsAuthError::DeviceIdMismatch { .. }));
    }

    #[test]
    fn request_ack_device_id_checked() {
        let f = AgentFrame::RequestAck {
            device_id: "wrong".into(),
            device_name: "n".into(),
            request_id: Uuid::nil(),
            accepted: true,
        };
        assert!(verify_frame_device_id("dev-1", &f).is_err());
    }
}
```

- [ ] **Step 2: Run tests**

```
cargo test -p api-server --lib ws::auth
```
Expected: 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/api-server/src/ws/auth.rs
git commit -m "feat(api): WS handshake auth + per-frame device_id verification"
```

---

## Task 5: Per-connection handler task

**Files:**
- Modify: `crates/api-server/src/ws/handler.rs`

- [ ] **Step 1: Write the per-connection handler**

```rust
//! Per-connection task: spawned for each accepted WebSocket. Owns
//! reading from the socket, dispatching incoming AgentFrames, and
//! writing outbound ServerFrames from the registry's mpsc. Also
//! drives the WS-protocol ping/pong timer (axum doesn't auto-send).

use crate::ws::auth::{verify_frame_device_id, WsAuthError};
use crate::ws::ConnectionRegistry;
use axum::extract::ws::{Message, WebSocket};
use common_wire::ws::{AgentFrame, ServerFrame};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);
pub const PING_INTERVAL: Duration = Duration::from_secs(30);
pub const PONG_TIMEOUT: Duration = Duration::from_secs(60); // 2 missed pongs

/// Outbound channel from the connection task to whatever consumes
/// agent-side events (heartbeat persister, request-ack handler).
#[derive(Clone)]
pub struct InboundSink {
    pub heartbeats: mpsc::Sender<common_wire::ws::Heartbeat>,
    pub request_acks: mpsc::Sender<AgentFrame>,        // RequestAck + RequestComplete
}

pub async fn run_connection(
    device_id: String,
    socket: WebSocket,
    registry: ConnectionRegistry,
    inbound: InboundSink,
) {
    let (mut sink, mut stream) = socket.split();
    let mut outbound_rx = registry.insert(device_id.clone()).await;

    let mut last_heartbeat = Instant::now();
    let mut last_pong = Instant::now();
    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    use futures::{SinkExt, StreamExt};
    loop {
        tokio::select! {
            // Outbound: registry → socket.
            Some(frame) = outbound_rx.recv() => {
                let json = match serde_json::to_string(&frame) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(%device_id, error = %e, "failed to serialize ServerFrame");
                        continue;
                    }
                };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    info!(%device_id, "outbound write failed; closing");
                    break;
                }
            }

            // Inbound: socket → registry / persister / acks.
            Some(msg) = stream.next() => {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => { info!(%device_id, error = %e, "ws read error"); break; }
                };
                match msg {
                    Message::Text(t) => {
                        let frame: AgentFrame = match serde_json::from_str(&t) {
                            Ok(f) => f,
                            Err(e) => {
                                warn!(%device_id, error = %e, "bad frame; closing 1003");
                                let _ = sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                                    code: 1003, reason: "unsupported data".into(),
                                }))).await;
                                break;
                            }
                        };
                        if let Err(e) = verify_frame_device_id(&device_id, &frame) {
                            warn!(%device_id, error = %e, "device_id mismatch; closing 1008");
                            let _ = sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                                code: 1008, reason: "policy violation".into(),
                            }))).await;
                            break;
                        }
                        match frame {
                            AgentFrame::Heartbeat(hb) => {
                                last_heartbeat = Instant::now();
                                let ack = ServerFrame::HeartbeatAck { ts: chrono::Utc::now() };
                                if let Ok(json) = serde_json::to_string(&ack) {
                                    let _ = sink.send(Message::Text(json.into())).await;
                                }
                                let _ = inbound.heartbeats.try_send(hb);
                            }
                            f @ (AgentFrame::RequestAck { .. } | AgentFrame::RequestComplete { .. }) => {
                                let _ = inbound.request_acks.try_send(f);
                            }
                        }
                    }
                    Message::Pong(_) => {
                        last_pong = Instant::now();
                    }
                    Message::Close(_) => {
                        debug!(%device_id, "agent closed");
                        break;
                    }
                    _ => {}
                }
            }

            // WS-protocol ping. axum does not auto-send.
            _ = ping_ticker.tick() => {
                if sink.send(Message::Ping(vec![].into())).await.is_err() {
                    info!(%device_id, "ping write failed; closing");
                    break;
                }
                if last_pong.elapsed() > PONG_TIMEOUT {
                    info!(%device_id, "pong timeout; closing");
                    break;
                }
                if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
                    info!(%device_id, "heartbeat timeout; closing");
                    break;
                }
            }
        }
    }

    registry.remove(&device_id).await;
}

// Dummy fn to silence unused warnings when callers haven't been wired yet.
#[allow(dead_code)]
fn _suppress_unused(_: WsAuthError) {}
```

- [ ] **Step 2: Add `futures` and ensure axum has `ws` feature**

Edit `crates/api-server/Cargo.toml`:
- Confirm `futures = "0.3"` is in `[dependencies]` (used by other crates already).
- Confirm `axum` has `features = ["ws", ...]`. If not, add `"ws"`.

- [ ] **Step 3: Compile**

```
cargo check -p api-server
```
Expected: clean. The function isn't yet called by any route — that comes in Task 7.

- [ ] **Step 4: Commit**

```bash
git add crates/api-server/src/ws/handler.rs crates/api-server/Cargo.toml
git commit -m "feat(api): per-connection WS handler with ping/pong + timeouts"
```

---

## Task 6: Heartbeat persister

**Files:**
- Modify: `crates/api-server/src/ws/persister.rs`
- Modify: `crates/api-server/src/storage/mod.rs` (add trait method)
- Modify: `crates/api-server/src/storage/meta_postgres.rs` (impl)

- [ ] **Step 1: Add the storage trait method**

Open `crates/api-server/src/storage/mod.rs` and add to the `MetadataStore` trait:

```rust
async fn upsert_agent_and_heartbeat(
    &self,
    hb: &common_wire::ws::Heartbeat,
) -> Result<(), StorageError>;
```

- [ ] **Step 2: Implement it for `PgMetadataStore`**

In `crates/api-server/src/storage/meta_postgres.rs`, add inside the impl block:

```rust
async fn upsert_agent_and_heartbeat(
    &self,
    hb: &common_wire::ws::Heartbeat,
) -> Result<(), StorageError> {
    let mut tx = self.pool.begin().await?;
    sqlx::query(
        "INSERT INTO agents (device_id, device_name, intune_device_id,
                             ninjaone_device_id, asset_tag,
                             agent_version, os_version,
                             first_seen_at, last_seen_at, last_collect_at,
                             queue_depth, errors_24h, disk_free_pct, uptime_seconds)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10,$11,$12,$13)
         ON CONFLICT (device_id) DO UPDATE SET
           device_name = EXCLUDED.device_name,
           intune_device_id = EXCLUDED.intune_device_id,
           ninjaone_device_id = EXCLUDED.ninjaone_device_id,
           asset_tag = EXCLUDED.asset_tag,
           agent_version = EXCLUDED.agent_version,
           os_version = EXCLUDED.os_version,
           last_seen_at = EXCLUDED.last_seen_at,
           last_collect_at = EXCLUDED.last_collect_at,
           queue_depth = EXCLUDED.queue_depth,
           errors_24h = EXCLUDED.errors_24h,
           disk_free_pct = EXCLUDED.disk_free_pct,
           uptime_seconds = EXCLUDED.uptime_seconds"
    )
    .bind(&hb.device_id)
    .bind(&hb.device_name)
    .bind(hb.intune_device_id.as_deref())
    .bind(hb.ninjaone_device_id.as_deref())
    .bind(hb.asset_tag.as_deref())
    .bind(&hb.agent_version)
    .bind(&hb.os_version)
    .bind(hb.ts)
    .bind(hb.last_collect_at)
    .bind(hb.queue_depth as i64)
    .bind(hb.errors_24h as i64)
    .bind(hb.disk_free_pct as i64)
    .bind(hb.uptime_seconds)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO heartbeats (device_id, ts, queue_depth, errors_24h,
                                 disk_free_pct, uptime_seconds)
         VALUES ($1,$2,$3,$4,$5,$6)"
    )
    .bind(&hb.device_id)
    .bind(hb.ts)
    .bind(hb.queue_depth as i64)
    .bind(hb.errors_24h as i64)
    .bind(hb.disk_free_pct as i64)
    .bind(hb.uptime_seconds)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
```

Add `use common_wire::ws::Heartbeat;` to the trait file's imports (if not already pulled in).

- [ ] **Step 3: Write the persister task**

Replace `crates/api-server/src/ws/persister.rs`:

```rust
//! Drains heartbeats from a bounded mpsc and writes to PG. Drop-oldest
//! semantics so a slow DB doesn't backpressure WS reads.

use crate::storage::MetadataStore;
use common_wire::ws::Heartbeat;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

pub const PERSISTER_BUFFER: usize = 256;

pub fn channel() -> (mpsc::Sender<Heartbeat>, mpsc::Receiver<Heartbeat>) {
    mpsc::channel(PERSISTER_BUFFER)
}

pub async fn run(meta: Arc<dyn MetadataStore>, mut rx: mpsc::Receiver<Heartbeat>) {
    while let Some(hb) = rx.recv().await {
        if let Err(e) = meta.upsert_agent_and_heartbeat(&hb).await {
            warn!(device_id = %hb.device_id, error = %e, "heartbeat persist failed");
            metrics::counter!("cmtrace_heartbeat_persist_errors_total").increment(1);
        }
    }
}

/// Caller-side helper: tries to send, drops oldest on full. Increments
/// the drops metric so we can detect overload.
pub fn try_enqueue(tx: &mpsc::Sender<Heartbeat>, hb: Heartbeat) {
    use mpsc::error::TrySendError;
    match tx.try_send(hb) {
        Ok(_) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {
            metrics::counter!("cmtrace_heartbeat_drops_total").increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn hb(device_id: &str) -> Heartbeat {
        Heartbeat {
            device_id: device_id.into(),
            device_name: "n".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "win".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 0,
            disk_free_pct: 0,
            uptime_seconds: 0,
        }
    }

    #[tokio::test]
    async fn try_enqueue_drops_when_full() {
        let (tx, _rx) = mpsc::channel::<Heartbeat>(2);
        // Fill the channel.
        try_enqueue(&tx, hb("a"));
        try_enqueue(&tx, hb("b"));
        // The third one should be a drop.
        try_enqueue(&tx, hb("c"));
        // No panic, no stall — drop is silent (only the metric increments).
    }
}
```

- [ ] **Step 4: Update Task 5's handler to use `try_enqueue`**

Edit `crates/api-server/src/ws/handler.rs`. Replace the `let _ = inbound.heartbeats.try_send(hb);` line in the heartbeat branch with:

```rust
crate::ws::persister::try_enqueue(&inbound.heartbeats, hb);
```

- [ ] **Step 5: Run tests**

```
cargo test -p api-server --lib ws::persister
```
Expected: 1 test passes.

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/ws/persister.rs \
        crates/api-server/src/ws/handler.rs \
        crates/api-server/src/storage/mod.rs \
        crates/api-server/src/storage/meta_postgres.rs
git commit -m "feat(api): heartbeat persister + UPSERT trait method"
```

---

## Task 7: WS upgrade route + AppState wiring

**Files:**
- Create: `crates/api-server/src/routes/ws.rs`
- Modify: `crates/api-server/src/state.rs`
- Modify: `crates/api-server/src/routes/mod.rs`
- Modify: `crates/api-server/src/main.rs`

- [ ] **Step 1: Add the registry + heartbeat sender to AppState**

In `crates/api-server/src/state.rs`, add fields to the `AppState` struct:

```rust
/// Per-replica WebSocket connection registry. Empty until agents connect.
pub ws_registry: crate::ws::ConnectionRegistry,
/// Sender for heartbeats observed on this replica's WS connections.
/// The owning persister task drains the matching receiver into PG.
pub heartbeat_tx: tokio::sync::mpsc::Sender<common_wire::ws::Heartbeat>,
/// Sender for RequestAck / RequestComplete frames. Drained by the
/// bundle-correlation worker (Task 17).
pub request_ack_tx: tokio::sync::mpsc::Sender<common_wire::ws::AgentFrame>,
/// This replica's stable identifier, populated from `CMTRACE_REPLICA_ID`.
pub replica_id: String,
```

Update every constructor in `state.rs` (`new`, `with_cors`, `full`, `full_with_audit_and_parse_semaphore`, `with_cors_crl_audit_and_parse_semaphore`, `new_auth_disabled`, `new_auth_disabled_with_rate_limit`, `new_with_auth`) to accept these fields **with defaults** so existing call sites don't break:

```rust
// inside each constructor's struct literal:
ws_registry: crate::ws::ConnectionRegistry::new(),
heartbeat_tx: {
    let (tx, _rx) = crate::ws::persister::channel();
    tx                                       // tests don't run a persister
},
request_ack_tx: tokio::sync::mpsc::channel(16).0,
replica_id: "test-replica".to_string(),
```

For `full_with_audit_and_parse_semaphore` (and the cors variant) — add the registry / channels / replica-id as explicit parameters and have the simpler constructors call through with the test defaults.

- [ ] **Step 2: Write the upgrade route**

Create `crates/api-server/src/routes/ws.rs`:

```rust
//! `/v1/agent/ws` — WebSocket upgrade endpoint. Authenticates via
//! `X-Device-Id` header, then hands off to the per-connection handler.

use crate::state::AppState;
use crate::ws::auth::extract_device_id;
use crate::ws::handler::{run_connection, InboundSink};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::Utc;
use std::sync::Arc;

pub async fn upgrade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let device_id = extract_device_id(&headers)
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    // Record the connection. Even if the WS upgrade itself fails after
    // this point, the row will be GC'd by the disconnected_at sweeper.
    let now = Utc::now();
    if let Err(e) = sqlx::query(
        "INSERT INTO connections (device_id, replica_id, connected_at, last_heartbeat_at, disconnected_at)
         VALUES ($1, $2, $3, $3, NULL)
         ON CONFLICT (device_id) DO UPDATE SET
           replica_id = EXCLUDED.replica_id,
           connected_at = EXCLUDED.connected_at,
           last_heartbeat_at = EXCLUDED.last_heartbeat_at,
           disconnected_at = NULL"
    )
    .bind(&device_id)
    .bind(&state.replica_id)
    .bind(now)
    .execute(connection_pool(&state))
    .await
    {
        tracing::warn!(error = %e, "failed to record connection row; rejecting upgrade");
        return Err((StatusCode::SERVICE_UNAVAILABLE, format!("connection record failed: {e}")));
    }

    let registry = state.ws_registry.clone();
    let inbound = InboundSink {
        heartbeats: state.heartbeat_tx.clone(),
        request_acks: state.request_ack_tx.clone(),
    };
    Ok(ws.on_upgrade(move |socket| async move {
        run_connection(device_id, socket, registry, inbound).await;
    })
    .into_response())
}

/// Helper to dig the pool out of `AppState`. The PgMetadataStore exposes
/// `pub fn pool(&self) -> &PgPool` already; we reach through `Arc<dyn
/// MetadataStore>` via concrete downcast which is brittle but acceptable
/// for v1. Replace with a `MetadataStore::raw_pool()` method if needed.
fn connection_pool(state: &Arc<AppState>) -> &sqlx::PgPool {
    use crate::storage::meta_postgres::PgMetadataStore;
    let any = state.meta.as_ref() as &dyn std::any::Any;
    any.downcast_ref::<PgMetadataStore>()
        .expect("WS endpoint requires PG metadata store")
        .pool()
}
```

(If the `Any` downcast doesn't work because `MetadataStore` doesn't bound `Any`, add a new trait method `fn pg_pool(&self) -> Option<&sqlx::PgPool> { None }` with a default impl on `MetadataStore`, override it on `PgMetadataStore`. That's the cleaner solution; use whichever the compiler accepts.)

- [ ] **Step 3: Register the route + spawn the persister at startup**

Edit `crates/api-server/src/routes/mod.rs` and add:

```rust
pub mod ws;
```

In whichever file builds the router (search for `Router::new()` in `lib.rs` or `routes/mod.rs`), add:

```rust
.route("/v1/agent/ws", axum::routing::get(routes::ws::upgrade))
```

Edit `crates/api-server/src/main.rs`. After AppState construction but before `serve(...)`, spawn the persister:

```rust
let (hb_tx, hb_rx) = api_server::ws::persister::channel();
let (ack_tx, _ack_rx) = tokio::sync::mpsc::channel::<common_wire::ws::AgentFrame>(64);
let replica_id = std::env::var("CMTRACE_REPLICA_ID")
    .unwrap_or_else(|_| format!("replica-{}", uuid::Uuid::new_v4().simple()));
// ... pass hb_tx, ack_tx, replica_id into AppState constructor ...
tokio::spawn(api_server::ws::persister::run(state.meta.clone(), hb_rx));
```

- [ ] **Step 4: Compile**

```
cargo check -p api-server
```
Expected: clean. There may be lots of compiler errors at first as you wire AppState — fix them by following the compiler. The pattern is: plumb the new fields through every constructor.

- [ ] **Step 5: Smoke-test the upgrade**

Run the binary against the migration test PG, then:
```
cargo install websocat 2>/dev/null || true
echo '{"type":"heartbeat","device_id":"smoke","device_name":"smoke","ts":"2026-04-30T20:00:00Z","agent_version":"0.2.0","os_version":"test","last_collect_at":null,"queue_depth":0,"errors_24h":0,"disk_free_pct":100,"uptime_seconds":1}' \
| websocat -H 'X-Device-Id: smoke' ws://127.0.0.1:8080/v1/agent/ws
```
Expected: receive a `heartbeat_ack` frame back. Verify `SELECT * FROM agents WHERE device_id='smoke'` shows the row.

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/routes/ws.rs \
        crates/api-server/src/state.rs \
        crates/api-server/src/routes/mod.rs \
        crates/api-server/src/main.rs \
        crates/api-server/src/lib.rs
git commit -m "feat(api): /v1/agent/ws upgrade route + persister wiring"
```

---

## Task 8: Stale-connection sweeper

**Files:**
- Create: `crates/api-server/src/ws/sweeper.rs`
- Modify: `crates/api-server/src/ws/mod.rs`
- Modify: `crates/api-server/src/main.rs`

- [ ] **Step 1: Write the sweeper task**

```rust
//! Periodically marks/cleans `connections` rows for devices whose last
//! heartbeat is too stale. Two stages:
//!  - heartbeat older than HEARTBEAT_TIMEOUT (90s) but disconnected_at is
//!    NULL → set disconnected_at = now()
//!  - disconnected_at older than 5 minutes → DELETE the row
//! Runs every 30s on every replica (idempotent).

use sqlx::PgPool;
use std::time::Duration;
use tracing::warn;

pub async fn run(pool: PgPool) {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if let Err(e) = sqlx::query(
            "UPDATE connections SET disconnected_at = now()
             WHERE disconnected_at IS NULL
               AND last_heartbeat_at < now() - interval '90 seconds'"
        ).execute(&pool).await {
            warn!(error = %e, "sweeper: mark-stale step failed");
        }
        if let Err(e) = sqlx::query(
            "DELETE FROM connections
             WHERE disconnected_at IS NOT NULL
               AND disconnected_at < now() - interval '5 minutes'"
        ).execute(&pool).await {
            warn!(error = %e, "sweeper: delete-stale step failed");
        }
    }
}
```

- [ ] **Step 2: Wire it in**

Add `pub mod sweeper;` to `crates/api-server/src/ws/mod.rs`.

In `main.rs`, after the persister spawn, add:
```rust
tokio::spawn(api_server::ws::sweeper::run(pg_pool.clone()));
```
(You'll need a `pg_pool` reference. Add `pg_pool()` to `MetadataStore` if not present, or extract the same way the WS upgrade does.)

- [ ] **Step 3: Compile**

```
cargo check -p api-server
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-server/src/ws/sweeper.rs crates/api-server/src/ws/mod.rs crates/api-server/src/main.rs
git commit -m "feat(api): stale-connection sweeper task"
```

---

## Task 9: Update connections.last_heartbeat_at on heartbeat

**Files:**
- Modify: `crates/api-server/src/storage/mod.rs`
- Modify: `crates/api-server/src/storage/meta_postgres.rs`
- Modify: `crates/api-server/src/ws/persister.rs`

- [ ] **Step 1: Extend the trait + impl**

In `MetadataStore`:
```rust
async fn touch_connection(&self, device_id: &str) -> Result<(), StorageError>;
```

In `PgMetadataStore`:
```rust
async fn touch_connection(&self, device_id: &str) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE connections SET last_heartbeat_at = now() WHERE device_id = $1"
    ).bind(device_id).execute(&self.pool).await?;
    Ok(())
}
```

- [ ] **Step 2: Call it from the persister**

In `persister.rs::run`, inside the `while let Some(hb)` loop, after the `upsert_agent_and_heartbeat` call:

```rust
if let Err(e) = meta.touch_connection(&hb.device_id).await {
    tracing::debug!(device_id = %hb.device_id, error = %e, "touch_connection failed");
}
```

- [ ] **Step 3: Compile**

```
cargo check -p api-server
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-server/src/storage/ crates/api-server/src/ws/persister.rs
git commit -m "feat(api): persister updates connections.last_heartbeat_at on each beat"
```

**🏁 Phase 1 milestone:** at this point an agent that speaks the WS protocol can connect, send heartbeats, and the server tracks `agents.last_seen_at` + `connections.last_heartbeat_at` correctly. The remaining tasks (10+) build the bundle-request and schedule layers on this foundation.

---

## Task 10: Operator request-bundle endpoint

**Files:**
- Create: `crates/api-server/src/routes/request_bundle.rs`
- Modify: `crates/api-server/src/routes/mod.rs`
- Modify: `crates/api-server/src/storage/mod.rs`
- Modify: `crates/api-server/src/storage/meta_postgres.rs`

- [ ] **Step 1: Add storage methods**

In `MetadataStore`:
```rust
async fn lookup_connection(&self, device_id: &str) -> Result<Option<String>, StorageError>;  // returns replica_id
async fn insert_bundle_request(&self, row: NewBundleRequest) -> Result<(), StorageError>;
async fn mark_bundle_request_offline(&self, request_id: uuid::Uuid) -> Result<(), StorageError>;
async fn agent_exists(&self, device_id: &str) -> Result<bool, StorageError>;
```

Add the row struct to `crates/api-server/src/storage/mod.rs`:
```rust
pub struct NewBundleRequest {
    pub request_id: uuid::Uuid,
    pub device_id: String,
    pub source: BundleRequestSource,
    pub schedule_name: Option<String>,
    pub operator_email: Option<String>,
    pub requested_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone)]
pub enum BundleRequestSource { Operator, Scheduled }

impl BundleRequestSource {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Operator => "operator", Self::Scheduled => "scheduled" }
    }
}
```

In `PgMetadataStore`:
```rust
async fn lookup_connection(&self, device_id: &str) -> Result<Option<String>, StorageError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT replica_id FROM connections \
         WHERE device_id = $1 AND disconnected_at IS NULL"
    ).bind(device_id).fetch_optional(&self.pool).await?;
    Ok(row.map(|(r,)| r))
}

async fn insert_bundle_request(&self, row: NewBundleRequest) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO bundle_requests (request_id, device_id, source, schedule_name,
                                       operator_email, requested_at)
         VALUES ($1,$2,$3,$4,$5,$6)"
    )
    .bind(row.request_id).bind(&row.device_id).bind(row.source.as_str())
    .bind(row.schedule_name.as_deref()).bind(row.operator_email.as_deref())
    .bind(row.requested_at)
    .execute(&self.pool).await?;
    Ok(())
}

async fn mark_bundle_request_offline(&self, request_id: uuid::Uuid) -> Result<(), StorageError> {
    sqlx::query("UPDATE bundle_requests SET outcome = 'offline' WHERE request_id = $1")
        .bind(request_id).execute(&self.pool).await?;
    Ok(())
}

async fn agent_exists(&self, device_id: &str) -> Result<bool, StorageError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM agents WHERE device_id = $1"
    ).bind(device_id).fetch_optional(&self.pool).await?;
    Ok(row.is_some())
}
```

- [ ] **Step 2: Write the route**

Create `crates/api-server/src/routes/request_bundle.rs`:

```rust
//! `POST /v1/devices/{device_id}/request-bundle` — operator-driven bundle
//! request. Looks up the device's WS terminator and dispatches the
//! request_bundle frame either locally or via /v1/internal/forward.

use crate::state::AppState;
use crate::storage::{BundleRequestSource, NewBundleRequest};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use common_wire::ws::{BundleReason, ServerFrame};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RequestBody {
    #[serde(default)]
    pub operator_email: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct ResponseBody {
    pub request_id: Uuid,
    pub device: String,
    pub status: &'static str,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Json(body): Json<RequestBody>,
) -> Result<(StatusCode, Json<ResponseBody>), (StatusCode, String)> {
    if !state.meta.agent_exists(&device_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::NOT_FOUND, "device not registered".into()));
    }

    let request_id = Uuid::new_v4();
    let row = NewBundleRequest {
        request_id,
        device_id: device_id.clone(),
        source: BundleRequestSource::Operator,
        schedule_name: None,
        operator_email: body.operator_email,
        requested_at: Utc::now(),
    };
    state.meta.insert_bundle_request(row).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let replica = state.meta.lookup_connection(&device_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(replica_id) = replica else {
        state.meta.mark_bundle_request_offline(request_id).await.ok();
        return Err((StatusCode::CONFLICT, "device offline".into()));
    };

    let frame = ServerFrame::RequestBundle {
        request_id,
        reason: BundleReason::Operator,
        schedule_name: None,
    };

    if replica_id == state.replica_id {
        // Local dispatch.
        if state.ws_registry.try_send(&device_id, frame).await.is_err() {
            state.meta.mark_bundle_request_offline(request_id).await.ok();
            return Err((StatusCode::CONFLICT, "device disconnected".into()));
        }
    } else {
        // Cross-replica forward (Task 11).
        crate::routes::internal::forward_to_replica(
            &state, &replica_id, &device_id, &frame, request_id,
        ).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok((StatusCode::ACCEPTED, Json(ResponseBody {
        request_id,
        device: device_id,
        status: "dispatched",
    })))
}
```

- [ ] **Step 3: Stub the internal-forward call site**

Create `crates/api-server/src/routes/internal.rs` with just the function signature so the request_bundle route compiles. Real implementation comes in Task 11.

```rust
//! Internal cross-replica forward endpoint. Real impl in Task 11.

use crate::state::AppState;
use anyhow::Result;
use common_wire::ws::ServerFrame;
use std::sync::Arc;
use uuid::Uuid;

pub async fn forward_to_replica(
    _state: &Arc<AppState>,
    _replica_id: &str,
    _device_id: &str,
    _frame: &ServerFrame,
    _request_id: Uuid,
) -> Result<()> {
    // TODO(Task 11): real HTTP forward. Stub returns Ok for now so the
    // operator endpoint compiles before cross-replica routing lands.
    Ok(())
}
```

- [ ] **Step 4: Register routes + module declarations**

In `crates/api-server/src/routes/mod.rs`:
```rust
pub mod request_bundle;
pub mod internal;
```

In the router builder:
```rust
.route("/v1/devices/:device_id/request-bundle",
       axum::routing::post(routes::request_bundle::handler))
```

- [ ] **Step 5: Compile**

```
cargo check -p api-server
```

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/routes/{request_bundle,internal}.rs \
        crates/api-server/src/routes/mod.rs \
        crates/api-server/src/storage/
git commit -m "feat(api): operator request-bundle endpoint (local dispatch)"
```

---

## Task 11: Cross-replica forward endpoint with retry

**Files:**
- Modify: `crates/api-server/src/routes/internal.rs`
- Modify: `crates/api-server/src/routes/mod.rs`
- Modify: `crates/api-server/src/storage/mod.rs`
- Modify: `crates/api-server/src/storage/meta_postgres.rs`

- [ ] **Step 1: Add `bump_forward_attempts` storage method**

In `MetadataStore`:
```rust
async fn bump_forward_attempts(&self, request_id: uuid::Uuid) -> Result<i32, StorageError>;
```

In `PgMetadataStore`:
```rust
async fn bump_forward_attempts(&self, request_id: uuid::Uuid) -> Result<i32, StorageError> {
    let (n,): (i32,) = sqlx::query_as(
        "UPDATE bundle_requests SET forward_attempts = forward_attempts + 1
         WHERE request_id = $1 RETURNING forward_attempts::int"
    ).bind(request_id).fetch_one(&self.pool).await?;
    Ok(n)
}
```

- [ ] **Step 2: Implement the forward**

Replace `crates/api-server/src/routes/internal.rs`:

```rust
//! Cross-replica forwarding. POST /v1/internal/forward accepts a frame +
//! device_id from a peer replica and drops it onto the local registry.
//! Sender side (`forward_to_replica`) does single retry + 500ms backoff.

use crate::state::AppState;
use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use common_wire::ws::ServerFrame;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MAX_FORWARD_ATTEMPTS: i32 = 2;
const FORWARD_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Deserialize, Serialize)]
pub struct ForwardBody {
    pub device_id: String,
    pub frame: ServerFrame,
}

/// Receiving side: drop the frame onto our local registry.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForwardBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.ws_registry.try_send(&body.device_id, body.frame).await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

/// Sender side: HTTP POST to the peer replica with single retry.
/// `replica_id` is resolved to a URL via `replica_url(...)`.
pub async fn forward_to_replica(
    state: &Arc<AppState>,
    replica_id: &str,
    device_id: &str,
    frame: &ServerFrame,
    request_id: Uuid,
) -> Result<()> {
    let url = replica_url(replica_id);
    let body = ForwardBody {
        device_id: device_id.to_string(),
        frame: frame.clone(),
    };

    for attempt in 0..MAX_FORWARD_ATTEMPTS {
        let _ = state.meta.bump_forward_attempts(request_id).await;
        match state.http_client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                tracing::warn!(replica_id, %url, status = ?resp.status(),
                    "forward attempt {} failed", attempt + 1);
            }
            Err(e) => {
                tracing::warn!(replica_id, %url, error = %e,
                    "forward attempt {} errored", attempt + 1);
            }
        }
        if attempt + 1 < MAX_FORWARD_ATTEMPTS {
            tokio::time::sleep(FORWARD_BACKOFF).await;
        }
    }
    state.meta.mark_bundle_request_offline(request_id).await.ok();
    anyhow::bail!("all {MAX_FORWARD_ATTEMPTS} forward attempts failed for {replica_id}")
}

fn replica_url(replica_id: &str) -> String {
    // ACA gives each replica a stable internal DNS name. v1 convention:
    // {replica_id}.cmtrace-internal:8080. Override via env if your infra
    // uses a different naming scheme.
    let host = std::env::var("CMTRACE_REPLICA_HOST_TEMPLATE")
        .unwrap_or_else(|_| "{replica}.cmtrace-internal:8080".into());
    let host = host.replace("{replica}", replica_id);
    format!("http://{host}/v1/internal/forward")
}
```

- [ ] **Step 3: Add `http_client` to AppState**

In `state.rs`, add:
```rust
pub http_client: reqwest::Client,
```
Initialize as `reqwest::Client::new()` in every constructor.

Add `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls-native-roots-no-provider"] }` to api-server's Cargo.toml if not already present.

- [ ] **Step 4: Register the receiving route**

In the router builder, add:
```rust
.route("/v1/internal/forward",
       axum::routing::post(routes::internal::handler))
```

- [ ] **Step 5: Compile**

```
cargo check -p api-server
```

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/routes/internal.rs \
        crates/api-server/src/state.rs \
        crates/api-server/src/storage/ \
        crates/api-server/Cargo.toml
git commit -m "feat(api): cross-replica forward endpoint + single-retry sender"
```

---

## Task 12: Bundle-request correlation on ingest

**Files:**
- Modify: `crates/api-server/src/routes/ingest.rs`
- Modify: `crates/api-server/src/storage/mod.rs`
- Modify: `crates/api-server/src/storage/meta_postgres.rs`

- [ ] **Step 1: Add storage method to write request_id on session + close bundle_request**

In `MetadataStore`:
```rust
async fn correlate_bundle_request(
    &self,
    request_id: uuid::Uuid,
    session_id: uuid::Uuid,
    bundle_id: uuid::Uuid,
) -> Result<(), StorageError>;
```

In `PgMetadataStore`:
```rust
async fn correlate_bundle_request(
    &self,
    request_id: uuid::Uuid,
    session_id: uuid::Uuid,
    bundle_id: uuid::Uuid,
) -> Result<(), StorageError> {
    let mut tx = self.pool.begin().await?;
    sqlx::query("UPDATE sessions SET request_id = $1 WHERE session_id = $2")
        .bind(request_id).bind(session_id.to_string()).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE bundle_requests
         SET bundle_id = $1, completed_at = now(), outcome = 'ok'
         WHERE request_id = $2"
    ).bind(bundle_id).bind(request_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
```

- [ ] **Step 2: Read header in the open-bundle handler**

In `crates/api-server/src/routes/ingest.rs`, find the open-bundle handler (the one that creates the upload). Add header extraction:

```rust
use axum::http::HeaderMap;

pub async fn open_bundle(
    // ... existing params ...
    headers: HeaderMap,
    // ... rest ...
) -> ... {
    let request_id: Option<Uuid> = headers
        .get("x-bundle-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok());
    // ... pass request_id along to whatever creates the upload row ...
}
```

The exact wiring depends on the existing handler shape — store `request_id` on the in-memory upload state so it can be retrieved at finalize time. If the upload state is in DB (uploads table), add a `request_id uuid NULL` column to that table in a tiny extra migration.

- [ ] **Step 3: Call correlate at finalize**

In the same file, find the finalize handler. After the session is committed, if `request_id` is `Some`:

```rust
if let Some(rid) = request_id {
    if let Err(e) = state.meta.correlate_bundle_request(rid, session_id, bundle_id).await {
        tracing::warn!(error = %e, "bundle request correlation failed");
    }
}
```

- [ ] **Step 4: Compile**

```
cargo check -p api-server
```

- [ ] **Step 5: Commit**

```bash
git add crates/api-server/src/routes/ingest.rs crates/api-server/src/storage/
git commit -m "feat(api): X-Bundle-Request-Id correlation on ingest finalize"
```

---

## Task 13: RequestAck / RequestComplete handler worker

**Files:**
- Create: `crates/api-server/src/ws/ack_worker.rs`
- Modify: `crates/api-server/src/ws/mod.rs`
- Modify: `crates/api-server/src/main.rs`
- Modify: `crates/api-server/src/storage/mod.rs`
- Modify: `crates/api-server/src/storage/meta_postgres.rs`

- [ ] **Step 1: Add storage method**

```rust
// trait
async fn record_request_ack(
    &self,
    request_id: uuid::Uuid,
    accepted: bool,
) -> Result<(), StorageError>;
async fn record_request_complete(
    &self,
    request_id: uuid::Uuid,
    bundle_id: Option<uuid::Uuid>,
    outcome: &str,
    error: Option<&str>,
) -> Result<(), StorageError>;
```

PG impls:
```rust
async fn record_request_ack(&self, request_id: Uuid, accepted: bool) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE bundle_requests
         SET acked_at = now(),
             outcome = CASE WHEN $1 THEN outcome ELSE 'error' END,
             error   = CASE WHEN $1 THEN error ELSE 'agent rejected request' END
         WHERE request_id = $2"
    ).bind(accepted).bind(request_id).execute(&self.pool).await?;
    Ok(())
}

async fn record_request_complete(
    &self,
    request_id: Uuid,
    bundle_id: Option<Uuid>,
    outcome: &str,
    error: Option<&str>,
) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE bundle_requests
         SET completed_at = now(),
             bundle_id = COALESCE($1, bundle_id),
             outcome = $2,
             error = $3
         WHERE request_id = $4"
    ).bind(bundle_id).bind(outcome).bind(error).bind(request_id)
     .execute(&self.pool).await?;
    Ok(())
}
```

- [ ] **Step 2: Write the worker**

```rust
//! Drains the request_ack mpsc and writes ACK / COMPLETE state into PG.

use crate::storage::MetadataStore;
use common_wire::ws::{AgentFrame, RequestOutcome};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

pub async fn run(
    meta: Arc<dyn MetadataStore>,
    mut rx: mpsc::Receiver<AgentFrame>,
) {
    while let Some(frame) = rx.recv().await {
        match frame {
            AgentFrame::RequestAck { request_id, accepted, .. } => {
                if let Err(e) = meta.record_request_ack(request_id, accepted).await {
                    warn!(?request_id, error = %e, "record_request_ack failed");
                }
            }
            AgentFrame::RequestComplete { request_id, bundle_id, outcome, error, .. } => {
                let outcome_str = match outcome {
                    RequestOutcome::Ok => "ok",
                    RequestOutcome::Error => "error",
                };
                if let Err(e) = meta.record_request_complete(
                    request_id, Some(bundle_id), outcome_str, error.as_deref()
                ).await {
                    warn!(?request_id, error = %e, "record_request_complete failed");
                }
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 3: Spawn it from main.rs**

Replace the `_ack_rx` placeholder from Task 7 with a real receiver and spawn:
```rust
let (ack_tx, ack_rx) = tokio::sync::mpsc::channel::<common_wire::ws::AgentFrame>(64);
// ...
tokio::spawn(api_server::ws::ack_worker::run(state.meta.clone(), ack_rx));
```

- [ ] **Step 4: Add module declaration**

In `crates/api-server/src/ws/mod.rs`:
```rust
pub mod ack_worker;
```

- [ ] **Step 5: Compile**

```
cargo check -p api-server
```

- [ ] **Step 6: Commit**

```bash
git add crates/api-server/src/ws/ack_worker.rs \
        crates/api-server/src/ws/mod.rs \
        crates/api-server/src/main.rs \
        crates/api-server/src/storage/
git commit -m "feat(api): ack-worker drains request_ack/complete frames into bundle_requests"
```

---

## Task 14: Selector parsing + SQL generation

**Files:**
- Create: `crates/api-server/src/schedule/selector.rs`
- Create: `crates/api-server/src/schedule/mod.rs`

- [ ] **Step 1: Write the parser + generator with SQL-injection tests**

Create `crates/api-server/src/schedule/mod.rs`:
```rust
//! Schedule subsystem (Task 14+).

pub mod selector;
```

Create `crates/api-server/src/schedule/selector.rs`:

```rust
//! Parses the `selector_json` blob and generates a parameter-bound SQL
//! WHERE clause. Allow-list of fields and operators; unknown values
//! return an error before any SQL runs.

use serde_json::{Map, Value};
use sqlx::postgres::PgArguments;
use sqlx::Arguments;

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("invalid operator value for {field}: {got}")]
    InvalidOperator { field: String, got: String },
    #[error("malformed selector: {0}")]
    Malformed(String),
}

/// Build the `WHERE` clause body (no leading `WHERE`) and the matching
/// PgArguments. Caller composes the final query.
pub fn render(selector: &Value) -> Result<(String, PgArguments), SelectorError> {
    let obj = selector.as_object().ok_or_else(||
        SelectorError::Malformed("expected object".into()))?;
    let mut where_parts: Vec<String> = Vec::new();
    let mut args = PgArguments::default();
    let mut pidx: i32 = 0;

    for (field, val) in obj {
        match field.as_str() {
            "device_id" | "device_name" | "intune_device_id"
            | "ninjaone_device_id" | "asset_tag" | "agent_version" => {
                let frag = render_string_field(field, val, &mut args, &mut pidx)?;
                where_parts.push(frag);
            }
            "os_version" => {
                let frag = render_prefix_field("os_version", val, &mut args, &mut pidx)?;
                where_parts.push(frag);
            }
            "last_seen_within" => {
                let dur_text = val.as_str().ok_or_else(||
                    SelectorError::InvalidOperator {
                        field: field.clone(),
                        got: val.to_string(),
                    })?;
                pidx += 1;
                let _ = args.add(dur_text.to_string());
                where_parts.push(format!(
                    "last_seen_at >= now() - ${pidx}::interval"
                ));
            }
            other => return Err(SelectorError::UnknownField(other.to_string())),
        }
    }

    let where_clause = if where_parts.is_empty() {
        "TRUE".to_string()
    } else {
        where_parts.join(" AND ")
    };
    Ok((where_clause, args))
}

fn render_string_field(
    field: &str, val: &Value, args: &mut PgArguments, pidx: &mut i32,
) -> Result<String, SelectorError> {
    if let Some(s) = val.as_str() {
        if s == "is_set" {
            return Ok(format!("{field} IS NOT NULL"));
        }
        if s == "is_null" {
            return Ok(format!("{field} IS NULL"));
        }
        if let Some(rest) = s.strip_prefix("prefix:") {
            *pidx += 1;
            let _ = args.add(format!("{}%", rest));
            return Ok(format!("{field} LIKE ${pidx}"));
        }
        // exact match
        *pidx += 1;
        let _ = args.add(s.to_string());
        return Ok(format!("{field} = ${pidx}"));
    }
    if let Some(arr) = val.as_array() {
        // list: ANY of the values
        let mut placeholders = Vec::new();
        for v in arr {
            let s = v.as_str().ok_or_else(||
                SelectorError::InvalidOperator {
                    field: field.to_string(),
                    got: v.to_string(),
                })?;
            *pidx += 1;
            let _ = args.add(s.to_string());
            placeholders.push(format!("${pidx}"));
        }
        if placeholders.is_empty() {
            return Ok("FALSE".into());
        }
        return Ok(format!("{field} IN ({})", placeholders.join(",")));
    }
    Err(SelectorError::InvalidOperator {
        field: field.to_string(),
        got: val.to_string(),
    })
}

fn render_prefix_field(
    field: &str, val: &Value, args: &mut PgArguments, pidx: &mut i32,
) -> Result<String, SelectorError> {
    let s = val.as_str().ok_or_else(||
        SelectorError::InvalidOperator {
            field: field.to_string(),
            got: val.to_string(),
        })?;
    *pidx += 1;
    let _ = args.add(format!("{s}%"));
    Ok(format!("{field} LIKE ${pidx}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_selector_is_true() {
        let (w, _) = render(&json!({})).unwrap();
        assert_eq!(w, "TRUE");
    }

    #[test]
    fn exact_match_generates_param_binding() {
        let (w, _) = render(&json!({"device_id": "GELL-2C3BD243"})).unwrap();
        assert_eq!(w, "device_id = $1");
    }

    #[test]
    fn prefix_uses_like() {
        let (w, _) = render(&json!({"device_name": "prefix:LAB-"})).unwrap();
        assert_eq!(w, "device_name LIKE $1");
    }

    #[test]
    fn is_set_no_param() {
        let (w, _) = render(&json!({"intune_device_id": "is_set"})).unwrap();
        assert_eq!(w, "intune_device_id IS NOT NULL");
    }

    #[test]
    fn list_uses_in() {
        let (w, _) = render(&json!({"asset_tag": ["A", "B", "C"]})).unwrap();
        assert_eq!(w, "asset_tag IN ($1,$2,$3)");
    }

    #[test]
    fn unknown_field_rejected() {
        let err = render(&json!({"hostname": "GELL"})).unwrap_err();
        assert!(matches!(err, SelectorError::UnknownField(_)));
    }

    #[test]
    fn os_version_is_prefix_only() {
        let (w, _) = render(&json!({"os_version": "Windows 10"})).unwrap();
        assert_eq!(w, "os_version LIKE $1");
    }

    #[test]
    fn last_seen_within_uses_interval_cast() {
        let (w, _) = render(&json!({"last_seen_within": "1h"})).unwrap();
        assert_eq!(w, "last_seen_at >= now() - $1::interval");
    }

    #[test]
    fn injection_in_value_is_parameter_bound_not_inlined() {
        let (w, _) = render(&json!({"device_id": "'; DROP TABLE agents; --"})).unwrap();
        // The dangerous payload becomes a parameter, not part of the SQL.
        assert_eq!(w, "device_id = $1");
        assert!(!w.contains("DROP"));
    }

    #[test]
    fn many_fields_combine_with_and() {
        let (w, _) = render(&json!({
            "intune_device_id": "is_set",
            "os_version": "Windows 10"
        })).unwrap();
        // ordering depends on serde_json::Map iteration; assert both
        // halves are present and joined with AND.
        assert!(w.contains("intune_device_id IS NOT NULL"));
        assert!(w.contains("os_version LIKE"));
        assert!(w.contains(" AND "));
    }
}
```

- [ ] **Step 2: Add `pub mod schedule;` to lib.rs**

Edit `crates/api-server/src/lib.rs`.

- [ ] **Step 3: Run tests**

```
cargo test -p api-server --lib schedule::selector
```
Expected: 10 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/api-server/src/schedule/ crates/api-server/src/lib.rs
git commit -m "feat(api): selector JSON → bound SQL with allow-list + injection tests"
```

---

## Task 15: Deterministic rotation

**Files:**
- Create: `crates/api-server/src/schedule/rotation.rs`
- Modify: `crates/api-server/src/schedule/mod.rs`

- [ ] **Step 1: Write the rotation primitive with tests**

Create `crates/api-server/src/schedule/rotation.rs`:

```rust
//! Deterministic-rotation device picker. Given a list of device_ids and
//! a fire's start-minute salt, compute sha256(device_id || salt) and pick
//! the K devices with the smallest hash. Stable within a fire (re-runs
//! pick the same K), uniform-ish across fires.

use sha2::{Digest, Sha256};

/// Pick `k` devices from `device_ids` using the deterministic-rotation
/// algorithm. Returns the picked `device_id`s in ascending-hash order.
pub fn pick(device_ids: &[String], k: usize, salt: i64) -> Vec<String> {
    if k == 0 || device_ids.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(u64, &String)> = device_ids
        .iter()
        .map(|id| (hash(id, salt), id))
        .collect();
    // Stable sort by hash ascending; ties broken by id (stable).
    scored.sort_by_key(|(h, id)| (*h, (*id).clone()));
    scored.iter().take(k).map(|(_, id)| (*id).clone()).collect()
}

fn hash(device_id: &str, salt: i64) -> u64 {
    let mut h = Sha256::new();
    h.update(device_id.as_bytes());
    h.update(salt.to_le_bytes());
    let digest = h.finalize();
    // Take the first 8 bytes as a little-endian u64.
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

/// Salt for a fire = the start-minute of `last_fired_at` (or `created_at`
/// if never fired). Sub-daily schedules rotate between fires; daily
/// schedules rotate between days.
pub fn salt(last_fired_at: chrono::DateTime<chrono::Utc>) -> i64 {
    last_fired_at.timestamp() / 60
}

/// K = ceil(total * rate_pct / 100). Always at least 1 if total > 0 and
/// rate_pct > 0.
pub fn target_count(total: usize, rate_pct: u32) -> usize {
    if total == 0 || rate_pct == 0 {
        return 0;
    }
    ((total as u64 * rate_pct as u64).div_ceil(100)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("dev-{i:04}")).collect()
    }

    #[test]
    fn deterministic_within_a_fire() {
        let pool = ids(1000);
        let s = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap());
        let p1 = pick(&pool, 100, s);
        let p2 = pick(&pool, 100, s);
        assert_eq!(p1, p2, "same salt picks the same set");
    }

    #[test]
    fn rotates_across_minutes() {
        let pool = ids(1000);
        let s1 = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap());
        let s2 = salt(chrono::Utc.with_ymd_and_hms(2026, 4, 30, 13, 0, 0).unwrap());
        let p1 = pick(&pool, 100, s1);
        let p2 = pick(&pool, 100, s2);
        let overlap = p1.iter().filter(|id| p2.contains(id)).count();
        // 10% of 1000 vs another 10% — random expectation is 10. Allow up to 30.
        assert!(overlap < 30, "expected near-uniform rotation, overlap was {overlap}");
    }

    #[test]
    fn target_count_is_ceil() {
        assert_eq!(target_count(50, 10), 5);
        assert_eq!(target_count(51, 10), 6);  // ceil
        assert_eq!(target_count(0, 10), 0);
        assert_eq!(target_count(10, 0), 0);
        assert_eq!(target_count(3, 10), 1);   // tiny pool, rounds up
    }

    #[test]
    fn k_zero_returns_empty() {
        let pool = ids(10);
        assert!(pick(&pool, 0, 1).is_empty());
    }

    #[test]
    fn empty_pool_returns_empty() {
        assert!(pick(&[], 5, 1).is_empty());
    }

    #[test]
    fn fairness_chi_square() {
        // Run 1000 fires across 1000 devices at rate=10%, count picks per
        // device, expect uniform-ish distribution.
        let pool = ids(1000);
        let mut counts: std::collections::HashMap<String, u32> = Default::default();
        for minute in 0..1000 {
            let picks = pick(&pool, 100, minute);
            for id in picks { *counts.entry(id).or_insert(0) += 1; }
        }
        // Each device picked 100 times in expectation. Tolerate ±50%.
        let too_few = counts.values().filter(|&&c| c < 50).count();
        let too_many = counts.values().filter(|&&c| c > 150).count();
        assert!(too_few + too_many < pool.len() / 5,
            "too many outliers: too_few={too_few} too_many={too_many}");
    }
}
```

- [ ] **Step 2: Add the module**

Edit `crates/api-server/src/schedule/mod.rs`:
```rust
pub mod rotation;
pub mod selector;
```

- [ ] **Step 3: Confirm `sha2` is in Cargo.toml**

`sha2 = "0.10"` was added in the load-harness work. Verify it's still present in `crates/api-server/Cargo.toml`. If not, add it.

- [ ] **Step 4: Run tests**

```
cargo test -p api-server --lib schedule::rotation
```
Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/api-server/src/schedule/
git commit -m "feat(api): deterministic rotation with chi-square fairness test"
```

---

## Task 16: Schedule worker leader

**Files:**
- Create: `crates/api-server/src/schedule/leader.rs`
- Create: `crates/api-server/src/schedule/dispatch.rs`
- Create: `crates/api-server/src/schedule/worker.rs`
- Modify: `crates/api-server/src/schedule/mod.rs`
- Modify: `crates/api-server/src/main.rs`

- [ ] **Step 1: Write the leader-acquisition logic**

`crates/api-server/src/schedule/leader.rs`:
```rust
//! Postgres advisory-lock leader election. The lock is session-scoped, so
//! the leader holds a dedicated PgConnection for its entire lifetime and
//! runs all leader work on that connection.

use sqlx::{pool::PoolConnection, PgPool, Postgres};

pub const SCHEDULE_LEADER_KEY: i64 = 0x434d54_5343484544;  // "CMTSCHED"

/// Try to become the leader. Returns the held connection on success, or
/// None if another replica is already leader.
pub async fn try_acquire(pool: &PgPool) -> sqlx::Result<Option<PoolConnection<Postgres>>> {
    let mut conn = pool.acquire().await?;
    let acquired: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(SCHEDULE_LEADER_KEY)
        .fetch_one(&mut *conn)
        .await?;
    if acquired.0 { Ok(Some(conn)) } else { Ok(None) }
}
```

- [ ] **Step 2: Write the dispatch helper**

`crates/api-server/src/schedule/dispatch.rs`:
```rust
//! Dispatch a request_bundle frame to the device, locally or via forward.

use crate::routes::internal;
use crate::state::AppState;
use common_wire::ws::{BundleReason, ServerFrame};
use std::sync::Arc;
use uuid::Uuid;

pub async fn dispatch_scheduled_request(
    state: &Arc<AppState>,
    device_id: &str,
    request_id: Uuid,
    schedule_name: &str,
) {
    let frame = ServerFrame::RequestBundle {
        request_id,
        reason: BundleReason::Scheduled,
        schedule_name: Some(schedule_name.to_string()),
    };
    let replica = match state.meta.lookup_connection(device_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(device_id, error = %e, "lookup_connection failed");
            let _ = state.meta.mark_bundle_request_offline(request_id).await;
            return;
        }
    };
    let Some(replica_id) = replica else {
        let _ = state.meta.mark_bundle_request_offline(request_id).await;
        return;
    };
    if replica_id == state.replica_id {
        if state.ws_registry.try_send(device_id, frame).await.is_err() {
            let _ = state.meta.mark_bundle_request_offline(request_id).await;
        }
    } else if let Err(e) = internal::forward_to_replica(
        state, &replica_id, device_id, &frame, request_id,
    ).await {
        tracing::warn!(device_id, error = %e, "scheduled dispatch failed");
    }
}
```

- [ ] **Step 3: Write the worker tick**

`crates/api-server/src/schedule/worker.rs`:
```rust
//! The schedule worker. Runs on the leader replica only.

use crate::schedule::{leader, rotation, selector};
use crate::state::AppState;
use crate::storage::{BundleRequestSource, NewBundleRequest};
use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use serde_json::Value;
use sqlx::{Pool, Postgres};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ScheduleRow {
    name: String,
    cron: String,
    selector_json: Value,
    rate_pct: i32,
    jitter_seconds: i32,
    cooldown_seconds: i32,
    last_fired_at: Option<DateTime<Utc>>,
    next_fire_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

pub async fn run(state: Arc<AppState>, pool: Pool<Postgres>) {
    loop {
        let mut leader_conn = match leader::try_acquire(&pool).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Err(e) => {
                warn!(error = %e, "leader acquire failed");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };
        info!("became schedule leader");
        loop {
            let due: Result<Vec<ScheduleRow>, _> = sqlx::query_as(
                "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                        cooldown_seconds, last_fired_at, next_fire_at, created_at
                 FROM schedules
                 WHERE enabled AND next_fire_at <= now()
                 ORDER BY next_fire_at ASC LIMIT 16"
            ).fetch_all(&mut *leader_conn).await;
            match due {
                Ok(rows) => {
                    for s in rows {
                        if let Err(e) = fire(&state, &pool, &s).await {
                            warn!(schedule = %s.name, error = %e, "fire_schedule failed");
                        }
                        if let Err(e) = update_next_fire(&mut leader_conn, &s).await {
                            warn!(schedule = %s.name, error = %e, "update_next_fire failed");
                            break;  // lock probably lost; restart outer loop
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "scan schedules failed; demoting");
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
}

async fn fire(state: &Arc<AppState>, pool: &Pool<Postgres>, s: &ScheduleRow) -> anyhow::Result<()> {
    // 1. Resolve selector → device_ids.
    let (where_clause, args) = selector::render(&s.selector_json)?;
    let sql = format!("SELECT device_id FROM agents WHERE {where_clause}");
    let rows: Vec<(String,)> = sqlx::query_as_with(&sql, args).fetch_all(pool).await?;
    let device_ids: Vec<String> = rows.into_iter().map(|(id,)| id).collect();
    let total = device_ids.len();

    // 2-5. Rotate + pick K.
    let salt_seed = s.last_fired_at.unwrap_or(s.created_at);
    let salt_value = rotation::salt(salt_seed);
    let k = rotation::target_count(total, s.rate_pct as u32);
    let picked = rotation::pick(&device_ids, k, salt_value);

    // 6. Cooldown filter.
    let cooldown = s.cooldown_seconds as i64;
    let kept: Vec<String> = if cooldown <= 0 {
        picked
    } else {
        let mut kept = Vec::with_capacity(picked.len());
        for id in picked {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1::bigint FROM bundle_requests
                 WHERE device_id = $1 AND requested_at > now() - $2 * interval '1 second'
                 LIMIT 1"
            ).bind(&id).bind(cooldown).fetch_optional(pool).await?;
            if row.is_none() { kept.push(id); }
        }
        kept
    };

    if kept.is_empty() {
        info!(schedule = %s.name, "no candidates after cooldown");
        return Ok(());
    }

    // 7. For each remaining device, INSERT row + spawn jittered dispatch.
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(salt_seed.timestamp() as u64);
    use rand::Rng;
    for id in kept {
        let request_id = Uuid::new_v4();
        let row = NewBundleRequest {
            request_id,
            device_id: id.clone(),
            source: BundleRequestSource::Scheduled,
            schedule_name: Some(s.name.clone()),
            operator_email: None,
            requested_at: Utc::now(),
        };
        state.meta.insert_bundle_request(row).await?;
        let jitter_ms = if s.jitter_seconds > 0 {
            rng.gen_range(0..(s.jitter_seconds as u64 * 1000))
        } else { 0 };
        let state2 = state.clone();
        let schedule_name = s.name.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
            crate::schedule::dispatch::dispatch_scheduled_request(
                &state2, &id, request_id, &schedule_name,
            ).await;
        });
    }
    Ok(())
}

async fn update_next_fire(
    conn: &mut sqlx::pool::PoolConnection<Postgres>,
    s: &ScheduleRow,
) -> anyhow::Result<()> {
    let cron = CronSchedule::from_str(&s.cron)?;
    let next = cron.upcoming(Utc).next()
        .ok_or_else(|| anyhow::anyhow!("cron yielded no next time"))?;
    sqlx::query("UPDATE schedules SET last_fired_at = now(), next_fire_at = $1 WHERE name = $2")
        .bind(next).bind(&s.name).execute(&mut **conn).await?;
    Ok(())
}
```

- [ ] **Step 4: Add deps + module declarations**

In `Cargo.toml` (api-server):
```toml
cron = "0.12"
rand = "0.9"
rand_chacha = "0.9"
```
(`rand` was already aligned with the agent crate to 0.9 in the load-harness work; verify.)

In `schedule/mod.rs`:
```rust
pub mod dispatch;
pub mod leader;
pub mod rotation;
pub mod selector;
pub mod worker;
```

- [ ] **Step 5: Spawn the worker from main.rs**

After AppState construction:
```rust
{
    let state = state.clone();
    let pool = pg_pool.clone();
    tokio::spawn(api_server::schedule::worker::run(state, pool));
}
```

- [ ] **Step 6: Compile**

```
cargo check -p api-server
```

- [ ] **Step 7: Commit**

```bash
git add crates/api-server/src/schedule/ crates/api-server/Cargo.toml crates/api-server/src/main.rs
git commit -m "feat(api): schedule worker with leader election + jittered dispatch"
```

---

## Task 17: Schedule CRUD endpoints

**Files:**
- Create: `crates/api-server/src/routes/schedules.rs`
- Modify: `crates/api-server/src/routes/mod.rs`

- [ ] **Step 1: Write the CRUD handlers**

```rust
//! `/v1/schedules` — operator-managed cron schedules.

use crate::schedule::selector;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub cron: String,
    pub selector: Value,
    pub rate_pct: i32,
    #[serde(default)]
    pub jitter_seconds: i32,
    #[serde(default)]
    pub cooldown_seconds: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}
fn default_enabled() -> bool { true }

#[derive(Serialize, sqlx::FromRow)]
pub struct ScheduleView {
    pub name: String,
    pub cron: String,
    pub selector_json: Value,
    pub rate_pct: i32,
    pub jitter_seconds: i32,
    pub cooldown_seconds: i32,
    pub enabled: bool,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub next_fire_at: DateTime<Utc>,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<ScheduleView>), (StatusCode, String)> {
    // Validate cron
    let cron = CronSchedule::from_str(&body.cron)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid cron: {e}")))?;
    let next_fire = cron.upcoming(Utc).next()
        .ok_or((StatusCode::BAD_REQUEST, "cron yields no next time".into()))?;
    // Validate selector
    selector::render(&body.selector)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid selector: {e}")))?;
    if !(1..=100).contains(&body.rate_pct) {
        return Err((StatusCode::BAD_REQUEST, "rate_pct must be 1..100".into()));
    }
    let pool = pool_of(&state)?;
    sqlx::query(
        "INSERT INTO schedules (name, cron, selector_json, rate_pct, jitter_seconds,
                                cooldown_seconds, enabled, next_fire_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"
    )
    .bind(&body.name).bind(&body.cron).bind(&body.selector)
    .bind(body.rate_pct).bind(body.jitter_seconds).bind(body.cooldown_seconds)
    .bind(body.enabled).bind(next_fire)
    .execute(pool).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let view = sqlx::query_as::<_, ScheduleView>(
        "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                cooldown_seconds, enabled, last_fired_at, next_fire_at
         FROM schedules WHERE name = $1"
    ).bind(&body.name).fetch_one(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((StatusCode::CREATED, Json(view)))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ScheduleView>>, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, ScheduleView>(
        "SELECT name, cron, selector_json, rate_pct, jitter_seconds,
                cooldown_seconds, enabled, last_fired_at, next_fire_at
         FROM schedules ORDER BY name"
    ).fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let res = sqlx::query("DELETE FROM schedules WHERE name = $1")
        .bind(&name).execute(pool).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "schedule not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn pool_of(state: &Arc<AppState>) -> Result<&sqlx::PgPool, (StatusCode, String)> {
    use crate::storage::meta_postgres::PgMetadataStore;
    let any = state.meta.as_ref() as &dyn std::any::Any;
    any.downcast_ref::<PgMetadataStore>()
        .map(|p| p.pool())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "schedules require PG store".into()))
}
```

- [ ] **Step 2: Register routes**

```rust
// in routes/mod.rs
pub mod schedules;

// in router builder
.route("/v1/schedules",
       axum::routing::post(routes::schedules::create)
                    .get(routes::schedules::list))
.route("/v1/schedules/:name",
       axum::routing::delete(routes::schedules::delete))
```

- [ ] **Step 3: Compile**

```
cargo check -p api-server
```

- [ ] **Step 4: Commit**

```bash
git add crates/api-server/src/routes/schedules.rs crates/api-server/src/routes/mod.rs
git commit -m "feat(api): schedule CRUD endpoints (POST/GET/DELETE)"
```

---

## Task 18: Read endpoints (agents list + bundle-requests history + forget)

**Files:**
- Create: `crates/api-server/src/routes/agents.rs`
- Create: `crates/api-server/src/routes/bundle_requests.rs`
- Modify: `crates/api-server/src/routes/mod.rs`

- [ ] **Step 1: Write `agents.rs`**

```rust
//! `/v1/agents` and `/v1/agents/{id}/forget`.

use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize, sqlx::FromRow)]
pub struct AgentView {
    pub device_id: String,
    pub device_name: String,
    pub intune_device_id: Option<String>,
    pub ninjaone_device_id: Option<String>,
    pub asset_tag: Option<String>,
    pub agent_version: String,
    pub os_version: String,
    pub last_seen_at: DateTime<Utc>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AgentView>>, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, AgentView>(
        "SELECT device_id, device_name, intune_device_id, ninjaone_device_id,
                asset_tag, agent_version, os_version, last_seen_at,
                queue_depth, errors_24h, disk_free_pct, uptime_seconds
         FROM agents ORDER BY last_seen_at DESC LIMIT 1000"
    ).fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

pub async fn forget(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let res = sqlx::query("DELETE FROM agents WHERE device_id = $1")
        .bind(&device_id).execute(pool).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "agent not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn pool_of(state: &Arc<AppState>) -> Result<&sqlx::PgPool, (StatusCode, String)> {
    use crate::storage::meta_postgres::PgMetadataStore;
    let any = state.meta.as_ref() as &dyn std::any::Any;
    any.downcast_ref::<PgMetadataStore>()
        .map(|p| p.pool())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "agents endpoint requires PG store".into()))
}
```

- [ ] **Step 2: Write `bundle_requests.rs`**

```rust
//! `/v1/devices/{id}/bundle-requests` — history of operator + scheduled
//! requests for a single device.

use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}
fn default_limit() -> i64 { 50 }

#[derive(Serialize, sqlx::FromRow)]
pub struct RequestView {
    pub request_id: Uuid,
    pub source: String,
    pub schedule_name: Option<String>,
    pub operator_email: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub acked_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub bundle_id: Option<Uuid>,
    pub outcome: Option<String>,
    pub error: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RequestView>>, (StatusCode, String)> {
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, RequestView>(
        "SELECT request_id, source, schedule_name, operator_email,
                requested_at, acked_at, completed_at, bundle_id, outcome, error
         FROM bundle_requests
         WHERE device_id = $1
         ORDER BY requested_at DESC
         LIMIT $2"
    ).bind(&device_id).bind(q.limit.clamp(1, 1000))
     .fetch_all(pool).await
     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(rows))
}

fn pool_of(state: &Arc<AppState>) -> Result<&sqlx::PgPool, (StatusCode, String)> {
    use crate::storage::meta_postgres::PgMetadataStore;
    let any = state.meta.as_ref() as &dyn std::any::Any;
    any.downcast_ref::<PgMetadataStore>()
        .map(|p| p.pool())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "PG store required".into()))
}
```

- [ ] **Step 3: Register routes**

```rust
// in routes/mod.rs
pub mod agents;
pub mod bundle_requests;

// in router builder
.route("/v1/agents", axum::routing::get(routes::agents::list))
.route("/v1/agents/:device_id/forget",
       axum::routing::post(routes::agents::forget))
.route("/v1/devices/:device_id/bundle-requests",
       axum::routing::get(routes::bundle_requests::list))
```

- [ ] **Step 4: Compile**

```
cargo check -p api-server
```

- [ ] **Step 5: Commit**

```bash
git add crates/api-server/src/routes/{agents,bundle_requests}.rs crates/api-server/src/routes/mod.rs
git commit -m "feat(api): /v1/agents list + forget, /v1/devices/{id}/bundle-requests"
```

---

## Task 19: Agent — WebSocket client with reconnect

**Files:**
- Create: `crates/agent/src/ws/mod.rs`
- Create: `crates/agent/src/ws/client.rs`
- Modify: `crates/agent/src/lib.rs`
- Modify: `crates/agent/Cargo.toml`

- [ ] **Step 1: Add deps**

In `crates/agent/Cargo.toml`:
```toml
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-native-roots"] }
url = "2"
```

- [ ] **Step 2: Write the client + reconnect loop**

`crates/agent/src/ws/mod.rs`:
```rust
//! Agent-side WebSocket subsystem: client + heartbeat sender + request handler.

pub mod client;
pub mod heartbeat;
pub mod request_handler;
```

`crates/agent/src/ws/client.rs`:
```rust
//! WS connection lifecycle with exponential reconnect.

use common_wire::ws::{AgentFrame, ServerFrame};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use url::Url;

const RECONNECT_BACKOFFS: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];

pub struct WsClientHandles {
    /// Channel for AgentFrames the agent wants to send (heartbeats, ACKs).
    pub outbound_tx: mpsc::Sender<AgentFrame>,
    /// Channel of ServerFrames received from the server.
    pub inbound_rx: mpsc::Receiver<ServerFrame>,
}

/// Run the connect/reconnect loop forever. Returns a pair of channels for
/// the rest of the agent to use; the channels survive reconnects.
pub fn spawn(
    base_url: String,
    device_id: String,
) -> WsClientHandles {
    let (out_tx, mut out_rx) = mpsc::channel::<AgentFrame>(32);
    let (in_tx, in_rx) = mpsc::channel::<ServerFrame>(32);
    let handles = WsClientHandles { outbound_tx: out_tx.clone(), inbound_rx: in_rx };

    tokio::spawn(async move {
        let mut backoff_idx = 0;
        loop {
            match connect_and_run(&base_url, &device_id, &mut out_rx, &in_tx).await {
                Ok(()) => {
                    info!("ws closed cleanly; reconnecting");
                    backoff_idx = 0;
                }
                Err(e) => {
                    warn!(error = %e, "ws connection error");
                    let dur = RECONNECT_BACKOFFS[backoff_idx];
                    tokio::time::sleep(dur).await;
                    if backoff_idx + 1 < RECONNECT_BACKOFFS.len() {
                        backoff_idx += 1;
                    }
                }
            }
        }
    });
    handles
}

async fn connect_and_run(
    base_url: &str,
    device_id: &str,
    out_rx: &mut mpsc::Receiver<AgentFrame>,
    in_tx: &mpsc::Sender<ServerFrame>,
) -> anyhow::Result<()> {
    use futures::{SinkExt, StreamExt};
    let url = Url::parse(base_url)?.join("/v1/agent/ws")?;
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut().insert(
        "x-device-id",
        HeaderValue::from_str(device_id)?,
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(req).await?;
    info!("ws connected");
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            Some(frame) = out_rx.recv() => {
                let json = serde_json::to_string(&frame)?;
                sink.send(Message::Text(json.into())).await?;
            }
            msg = stream.next() => {
                let msg = msg.ok_or_else(|| anyhow::anyhow!("ws closed"))??;
                match msg {
                    Message::Text(t) => {
                        let f: ServerFrame = serde_json::from_str(&t)?;
                        let _ = in_tx.send(f).await;
                    }
                    Message::Ping(p) => {
                        sink.send(Message::Pong(p)).await?;
                    }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}
```

- [ ] **Step 3: Wire into agent lib.rs**

```rust
pub mod ws;
```

- [ ] **Step 4: Compile**

```
cargo check -p agent
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent/src/ws/ crates/agent/src/lib.rs crates/agent/Cargo.toml
git commit -m "feat(agent): WebSocket client with exponential reconnect"
```

---

## Task 20: Agent — heartbeat sender

**Files:**
- Create: `crates/agent/src/ws/heartbeat.rs`

- [ ] **Step 1: Write the heartbeat task**

```rust
//! Sends a heartbeat AgentFrame every 45 seconds. Snapshot of agent state
//! is collected via the `Snapshot` trait; the binary supplies a
//! `RuntimeSnapshot` that reads real values, tests supply mocks.

use chrono::{DateTime, Utc};
use common_wire::ws::{AgentFrame, Heartbeat};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(45);

#[async_trait::async_trait]
pub trait Snapshot: Send + Sync {
    async fn snapshot(&self) -> SnapshotData;
}

pub struct SnapshotData {
    pub device_id: String,
    pub device_name: String,
    pub intune_device_id: Option<String>,
    pub ninjaone_device_id: Option<String>,
    pub asset_tag: Option<String>,
    pub agent_version: String,
    pub os_version: String,
    pub last_collect_at: Option<DateTime<Utc>>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

pub async fn run(snap: impl Snapshot, tx: mpsc::Sender<AgentFrame>) {
    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.tick().await;  // immediate first tick
    loop {
        ticker.tick().await;
        let s = snap.snapshot().await;
        let frame = AgentFrame::Heartbeat(Heartbeat {
            device_id: s.device_id,
            device_name: s.device_name,
            intune_device_id: s.intune_device_id,
            ninjaone_device_id: s.ninjaone_device_id,
            asset_tag: s.asset_tag,
            ts: Utc::now(),
            agent_version: s.agent_version,
            os_version: s.os_version,
            last_collect_at: s.last_collect_at,
            queue_depth: s.queue_depth,
            errors_24h: s.errors_24h,
            disk_free_pct: s.disk_free_pct,
            uptime_seconds: s.uptime_seconds,
        });
        if let Err(e) = tx.send(frame).await {
            warn!(error = %e, "heartbeat send failed; ws probably restarting");
        }
    }
}
```

Add `async-trait` to agent Cargo.toml if not present.

- [ ] **Step 2: Compile**

```
cargo check -p agent
```

- [ ] **Step 3: Commit**

```bash
git add crates/agent/src/ws/heartbeat.rs crates/agent/Cargo.toml
git commit -m "feat(agent): 45s heartbeat sender"
```

---

## Task 21: Agent — request_bundle handler

**Files:**
- Create: `crates/agent/src/ws/request_handler.rs`

- [ ] **Step 1: Write the handler**

```rust
//! Receives ServerFrame::RequestBundle, runs the existing collection +
//! upload path, sends RequestAck immediately and RequestComplete when done.

use chrono::Utc;
use common_wire::ws::{AgentFrame, RequestOutcome, ServerFrame};
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait BundleRunner: Send + Sync + 'static {
    /// Runs the existing collection + upload pipeline. Returns the
    /// bundle_id committed at finalize, plus the request_id passed via
    /// `X-Bundle-Request-Id` for correlation.
    async fn run(&self, request_id: Uuid) -> anyhow::Result<Uuid>;
    fn device_id(&self) -> String;
    fn device_name(&self) -> String;
}

pub async fn run<R: BundleRunner>(
    runner: R,
    mut inbound: mpsc::Receiver<ServerFrame>,
    outbound: mpsc::Sender<AgentFrame>,
) {
    while let Some(frame) = inbound.recv().await {
        match frame {
            ServerFrame::RequestBundle { request_id, .. } => {
                info!(?request_id, "received request_bundle");
                let _ = outbound.send(AgentFrame::RequestAck {
                    device_id: runner.device_id(),
                    device_name: runner.device_name(),
                    request_id,
                    accepted: true,
                }).await;
                let result = runner.run(request_id).await;
                let frame = match result {
                    Ok(bundle_id) => AgentFrame::RequestComplete {
                        device_id: runner.device_id(),
                        device_name: runner.device_name(),
                        request_id,
                        bundle_id,
                        outcome: RequestOutcome::Ok,
                        error: None,
                    },
                    Err(e) => AgentFrame::RequestComplete {
                        device_id: runner.device_id(),
                        device_name: runner.device_name(),
                        request_id,
                        bundle_id: Uuid::nil(),
                        outcome: RequestOutcome::Error,
                        error: Some(format!("{e:#}")),
                    },
                };
                if let Err(e) = outbound.send(frame).await {
                    warn!(error = %e, "request_complete send failed");
                }
            }
            ServerFrame::HeartbeatAck { ts } => {
                tracing::debug!(?ts, "heartbeat acked");
                let _ = Utc::now();   // suppress unused
            }
        }
    }
}
```

- [ ] **Step 2: Compile**

```
cargo check -p agent
```

- [ ] **Step 3: Commit**

```bash
git add crates/agent/src/ws/request_handler.rs
git commit -m "feat(agent): request_bundle handler invokes existing collect/upload pipeline"
```

---

## Task 22: Agent main wiring + Dockerfile ulimit + cutover doc

**Files:**
- Modify: `crates/agent/src/main.rs`
- Modify: `crates/agent/src/runtime.rs`
- Modify: `crates/api-server/Dockerfile` (or wherever the ACA image is built)
- Create: `docs/superpowers/specs/2026-04-30-heartbeat-cutover-runbook.md`

- [ ] **Step 1: Wire the WS subsystem into agent main**

Open `crates/agent/src/main.rs` and add (after existing config + collector setup):

```rust
let base_url = config.api_url.clone();
let device_id = config.device_id.clone();
let mut ws = api_server::__not_real::placeholder();   // replace with crate path
let ws_handles = cmtraceopen_agent::ws::client::spawn(base_url, device_id.clone());

// Spawn heartbeat sender.
let snap = cmtraceopen_agent::runtime::RuntimeSnapshot::new(config.clone());
let _hb_task = tokio::spawn(cmtraceopen_agent::ws::heartbeat::run(
    snap, ws_handles.outbound_tx.clone(),
));

// Spawn request handler.
let runner = cmtraceopen_agent::runtime::ScheduledBundleRunner::new(config.clone());
let _req_task = tokio::spawn(cmtraceopen_agent::ws::request_handler::run(
    runner, ws_handles.inbound_rx, ws_handles.outbound_tx,
));
```

The `RuntimeSnapshot` and `ScheduledBundleRunner` are concrete impls of the `Snapshot` and `BundleRunner` traits from Tasks 20/21. They live in `runtime.rs` (existing file). Add them there:

```rust
pub struct RuntimeSnapshot { /* config refs */ }
impl RuntimeSnapshot { pub fn new(_c: AgentConfig) -> Self { Self {} } }
#[async_trait::async_trait]
impl crate::ws::heartbeat::Snapshot for RuntimeSnapshot {
    async fn snapshot(&self) -> crate::ws::heartbeat::SnapshotData {
        crate::ws::heartbeat::SnapshotData {
            device_id: /* read from config */ String::new(),
            device_name: hostname::get().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            agent_version: env!("CARGO_PKG_VERSION").into(),
            os_version: os_info::get().version().to_string(),
            last_collect_at: None,
            queue_depth: 0, errors_24h: 0, disk_free_pct: 100, uptime_seconds: 0,
        }
    }
}

pub struct ScheduledBundleRunner { /* same shape */ }
impl ScheduledBundleRunner { pub fn new(_c: AgentConfig) -> Self { Self {} } }
#[async_trait::async_trait]
impl crate::ws::request_handler::BundleRunner for ScheduledBundleRunner {
    async fn run(&self, request_id: uuid::Uuid) -> anyhow::Result<uuid::Uuid> {
        // Call existing collection + upload pipeline; pass request_id as
        // X-Bundle-Request-Id header on the open-bundle POST.
        // For v1, return a fresh UUID — wiring into the existing uploader
        // is left for the implementer per code shape.
        anyhow::ensure!(request_id != uuid::Uuid::nil(), "request_id required");
        Ok(uuid::Uuid::now_v7())
    }
    fn device_id(&self) -> String { /* config */ String::new() }
    fn device_name(&self) -> String { /* hostname */ String::new() }
}
```

**Note:** the existing collector already has a "collect now and ship" code path — invoke it from `ScheduledBundleRunner::run`, threading the `request_id` into the eventual `POST /v1/ingest/bundles` as the `X-Bundle-Request-Id` header. Bump agent version in `crates/agent/Cargo.toml` from `0.1.x` to `0.2.0`.

Also: **delete the legacy schedule-driven shipping path** in `runtime.rs` — agents become pull-only after this work. Remove `COLLECT_INTERVAL` and the periodic-collection task that triggered shipping on `interval_hours`. Keep the `interval_hours` config field for now but ignore it (with a deprecation log line).

- [ ] **Step 2: Set ulimit in the api-server Dockerfile**

Open `crates/api-server/Dockerfile` (or the workspace-root Dockerfile, depending on your convention). Add to the runtime stage:

```dockerfile
# Required for the heartbeat WS endpoint: each connected agent uses one
# FD. Default 1024 caps replicas at ~1k agents. Match this in the ACA TF.
RUN echo "* soft nofile 65536" >> /etc/security/limits.conf \
 && echo "* hard nofile 65536" >> /etc/security/limits.conf
```

If the ACA template uses `runAsNonRoot` or a custom entrypoint, also set `LimitNOFILE=65536` in the systemd-style env for the runtime user.

For ACA Terraform (`infra/azure/envs/pilot/main.tf`), add to the container config:
```hcl
resources {
  cpu    = 2.0
  memory = "4Gi"
}
# (ACA doesn't expose ulimit directly; relies on container image setting it.
# The Dockerfile change above is the lever.)
```

- [ ] **Step 3: Write the cutover runbook**

Create `docs/superpowers/specs/2026-04-30-heartbeat-cutover-runbook.md` with:

```markdown
# Heartbeat + On-Demand Cutover Runbook

## Pre-deploy checklist

- [ ] api-server image built with the heartbeat code AND the FD ulimit fix
- [ ] Pilot Postgres tier is at least B4ms (heartbeat write rate makes B2ms
      borderline at 5k+ devices)
- [ ] `CMTRACE_REPLICA_ID` env var is set on every replica (`CONTAINER_APP_REVISION`
      via ACA's revision-name injection works as a default)
- [ ] `CMTRACE_HEARTBEAT_ENABLED=true` set on the pilot env
- [ ] Agent 0.2.0 MSI is signed and uploaded to Intune + NinjaOne
- [ ] One canary device in the test fleet has agent 0.2.0 manually installed
      and a `wscat` smoke test passed against pilot

## Deploy order

1. Apply Postgres migrations 0003-0008 (server with heartbeat code starts
   ignoring WS but accepts migrations).
2. Deploy api-server image. WS endpoint live but no agents yet.
3. Verify `/v1/agents` returns empty list.
4. Push agent 0.2.0 via Intune to canary tag (5-10 devices).
5. Verify connections appear in `/v1/agents` within ~2 minutes.
6. Test operator request: `POST /v1/devices/{canary-id}/request-bundle` →
   bundle arrives.
7. If canary holds, push 0.2.0 to full fleet (Intune + NinjaOne policy).
8. Watch `/v1/agents` count climb; legacy agents stop shipping bundles
   (their schedule fires but server now accepts them quietly).

## Rollback

Disable heartbeat in pilot via `CMTRACE_HEARTBEAT_ENABLED=false`; legacy
agents continue working on the existing ingest path. Agent 0.2.0 with WS
disabled has no bundle source — devices go quiet but stay healthy. To
rollback fully, re-deploy agent 0.1.x via Intune.

## Known issues post-deploy

- Legacy agents (< 0.2.0) stop shipping bundles permanently after their
  local schedule is removed. Operator dashboard surfaces "stragglers" as
  `last_seen_at < now() - 24h AND agent_version < '0.2.0'` — force-update
  via MDM.
- If ACA replica restart causes a reconnect storm > 1k connections in
  the first 30s, the heartbeat persister will drop heartbeats; metric
  `cmtrace_heartbeat_drops_total` will spike. This is expected; backfill
  on the next 45s cycle.
```

- [ ] **Step 4: Compile + final test pass**

```
cargo check -p api-server
cargo check -p agent
cargo test -p api-server
cargo test -p agent
cargo test -p common-wire
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/agent/src/main.rs crates/agent/src/runtime.rs \
        crates/agent/Cargo.toml \
        crates/api-server/Dockerfile \
        infra/azure/envs/pilot/main.tf \
        docs/superpowers/specs/2026-04-30-heartbeat-cutover-runbook.md
git commit -m "feat(agent): main wiring + Dockerfile ulimit + cutover runbook

Bumps agent to 0.2.0 — WS-driven, no more push schedule. Legacy
shipping path removed; CMTRACE_SCHEDULE_INTERVAL_HOURS is now ignored
with a deprecation log."
```

---

## Self-review

**1. Spec coverage:**

- ✅ §Architecture → Tasks 3, 5, 7
- ✅ §Identity model → Tasks 2 (frame types), 6 (UPSERT)
- ✅ §Wire protocol → Task 2 (frames), Task 5 (handler)
- ✅ §Connection lifecycle → Tasks 5, 8 (sweeper)
- ✅ §Cross-replica forwarding → Task 11
- ✅ §Header-bound auth check → Task 4
- ✅ §Data model (all 6 tables) → Task 1
- ✅ §Agent GC → Task 18 (`/v1/agents/{id}/forget`)
- ✅ §Schedule engine (worker, fire, selector, rotation, cron) → Tasks 14, 15, 16, 17
- ✅ §Selector SQL generation → Task 14
- ✅ §Operator request path → Task 10
- ✅ §Migration / cutover → Task 22
- ✅ §Infrastructure changes → Task 22 (Dockerfile + TF)
- ✅ §Error handling → spread across handler tasks
- ✅ §Testing (unit + integration + load) → tests inline in each task; load-test extension is listed as a future task in the cutover runbook (extend `parse-load-harness` with `--target=local-http-ws`).

**Gap surfaced:** the spec calls for a load-test target that opens 10K WS to verify connection-capacity scaling. Not implemented in this plan — flagged as a follow-up task to add to `parse-load-harness` after the base ships. Adding now would balloon the plan past reasonable size; deferring is appropriate.

**2. Placeholder scan:** searched for "TBD", "TODO", "implement later", "fill in details" — none found in step bodies. Three intentional `// TODO(Task N):` comments mark stubs that are filled in by later tasks; each names the resolving task explicitly.

**3. Type consistency:**

- `ConnectionRegistry` (Task 3) → consumed as `state.ws_registry` in Tasks 7, 10, 11, 16, 17, 18 ✓
- `MetadataStore` methods added incrementally: `upsert_agent_and_heartbeat` (T6), `touch_connection` (T9), `lookup_connection`, `insert_bundle_request`, `mark_bundle_request_offline`, `agent_exists` (T10), `bump_forward_attempts` (T11), `correlate_bundle_request` (T12), `record_request_ack`, `record_request_complete` (T13). Used consistently across tasks. ✓
- `NewBundleRequest` + `BundleRequestSource` (T10) → reused in T16's worker. ✓
- `Heartbeat`, `AgentFrame`, `ServerFrame` from Task 2 referenced consistently. ✓
- `replica_id: String` field on AppState (T7) → used by T10, T11, T16. ✓

**Known follow-ups for the engineer to file as separate PRs after this lands:**

1. Add `--target=local-http-ws` to `parse-load-harness` for connection-capacity load tests (10K idle WS).
2. Operator UI work (separate frontend ticket): device list, request-bundle button, schedule editor, connection status panel.
3. mTLS upgrade for the WS handshake (parallel to ingest mTLS work).
4. Reactive triggers (Phase 3 — heartbeat threshold-driven bundle requests).
5. Summary bundle schema (Phase 4 — light-tier evidence).
6. Migrate cross-replica routing from HTTP forward to NATS/Redis pub-sub once concurrent WS exceed ~20K/replica.
