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

## What it does *today*

- Builds on Windows and Linux (Linux is a stub; the binary prints a
  "Windows only" message and exits 1).
- Loads `AgentConfig` from either a TOML file or `CMTRACE_*` env vars.
- Boots `tokio` + JSON-formatted `tracing`, logs a banner and the loaded
  config, and parks on ctrl-c.

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

- Windows service registration (`windows_service::service_dispatcher`).
- mTLS client cert loading.
- Collectors (`logs.rs`, `event_logs.rs`, `dsregcmd.rs`, `evidence.rs`).
- Upload queue (chunked / resumable).
- SQLite cursor / queue state DB.
- MSI packaging (WiX).
- HKLM registry overrides and ADMX policy ingestion.

Each of those is called out as a TODO comment in `src/main.rs` and will
land as its own PR.
