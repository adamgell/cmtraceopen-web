# cmtraceopen-agent

Windows service that ships logs and evidence from managed endpoints to the
CMTrace Open api-server.

This crate is the **scaffold** — the crate layout, config shape, and
Linux/Windows cfg-gating are in place, but none of the real collection or
upload logic lives here yet. See the TODO list at the top of `src/main.rs`
for the ordered plan.

## What it will do

On a managed Windows device (typically deployed via MSI → service):

1. Register as a Windows service and report status back to the SCM.
2. Load an mTLS client cert from `LocalMachine\My` (or a provisioned PFX
   under `%ProgramData%\CMTraceOpen\Agent`).
3. Collect:
   - ConfigMgr / Intune client logs (`ccmexec.log` and friends).
   - Windows Event Log channels relevant to device management.
   - `dsregcmd /status` output (Entra / AD join state).
   - Scheduled "evidence" pulls — full snapshots on a cron-like cadence.
4. Queue bundles to a local SQLite state DB
   (`%ProgramData%\CMTraceOpen\Agent\state.db`), chunk them, and upload to
   the api-server's ingest endpoint with resume-on-reconnect semantics.

## What it does *today* (Wave 2 M1)

- Builds on Windows and Linux via `#[cfg(target_os = "windows")]` gates.
- Loads `AgentConfig` from either a TOML file or `CMTRACE_*` env vars.
- **Collectors**: walks configured log glob paths; on Windows shells out
  to `wevtutil epl` for event logs and `dsregcmd /status` for join state.
  Linux stubs return `NotSupported` manifest entries so CI compiles.
- **Evidence orchestrator**: runs all collectors in parallel, zips the
  result, sha256-hashes the zip.
- **Persistent queue**: flat-file `{uuid}.zip` + `{uuid}.json` sidecars
  with atomic-rename writes under `%ProgramData%\cmtraceopen-agent\queue\`.
- **Uploader**: chunked resumable upload to the api-server's
  `/v1/ingest/bundles` protocol (init → chunks → finalize) with
  exponential-backoff retry (1s / 5s / 30s).
- **`--oneshot`**: collect + enqueue + drain once and exit. Default
  mode is a foreground daemon with interval-driven collection and a
  30-second drain loop.

## Config

Two sources, listed in order of precedence for `from_env_or_default`
(defaults < env). A full file load via `AgentConfig::from_file` replaces
all fields (with `#[serde(default)]` filling gaps).

| TOML key                | Env var                        | Default                         |
|-------------------------|--------------------------------|---------------------------------|
| `api_endpoint`          | `CMTRACE_API_ENDPOINT`         | `https://api.corp.example.com`  |
| `request_timeout_secs`  | `CMTRACE_REQUEST_TIMEOUT_SECS` | `60`                            |
| `evidence_schedule`     | `CMTRACE_EVIDENCE_SCHEDULE`    | `0 3 * * *`                     |
| `queue_max_bundles`     | `CMTRACE_QUEUE_MAX_BUNDLES`    | `50`                            |
| `log_level`             | `CMTRACE_LOG_LEVEL`            | `info`                          |
| `device_id`             | `CMTRACE_DEVICE_ID`            | *(hostname fallback)*           |
| `log_paths`             | *(no env override)*            | CCM + IME + DSRegCmd trees      |

The file is expected at `%ProgramData%\CMTraceOpen\Agent\config.toml` in
production, but `from_file` takes any `&Path`.

## Building

```bash
cargo check   -p agent
cargo build   -p agent
cargo clippy  -p agent --all-targets -- -D warnings
cargo test    -p agent
```

On Linux these all succeed; the `windows-service` / `windows` crates are
gated behind `cfg(target_os = "windows")` in `Cargo.toml`.

## Not yet in scope

- **Windows service registration** (`windows_service::service_dispatcher`).
  Main loop runs as a foreground daemon; `sc.exe create` + `sc.exe start`
  works for hand-testing until Wave 2 M2 wires the real SCM integration.
- mTLS client cert loading (Wave 3).
- TLS at the reqwest layer — see the big comment in `Cargo.toml`; the
  agent currently talks plaintext HTTP, so production deployments front
  the api-server with a TLS-terminating reverse proxy.
- SQLite cursor DB — flat-file queue is good enough for MVP.
- MSI packaging (WiX).
- HKLM registry overrides and ADMX policy ingestion.

Each of those is called out as a TODO comment in `src/main.rs` and will
land as its own PR.

## Running against a local api-server

```bash
# Terminal 1: api-server
cargo run -p api-server

# Terminal 2: one-shot agent against loopback
CMTRACE_API_ENDPOINT=http://127.0.0.1:8080 \
CMTRACE_DEVICE_ID=WIN-DEV-01 \
CMTRACE_QUEUE_MAX_BUNDLES=10 \
  cargo run -p agent -- --oneshot
```

The one-shot run collects once, enqueues, uploads, and exits. Check
`GET /v1/devices/WIN-DEV-01/sessions` for the resulting session.
