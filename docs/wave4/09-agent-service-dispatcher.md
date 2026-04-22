# Wave 4 / 09 — Agent Windows Service Dispatcher

**Status:** Implemented  
**Gating issue:** [feat(agent): Windows service dispatcher integration]

---

## Goal

Make the `cmtraceopen-agent` binary a proper Windows service so the SCM can
start and stop it without triggering spurious restart loops from the MSI
recovery action config.

---

## Design

### Mode detection

`main.rs` calls `cmtraceopen_agent::service::try_run_as_service()` **before**
any CLI parsing.  The function calls
`windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)`:

| Result | Meaning | Action |
|--------|---------|--------|
| `Ok(())` | Ran as service, stopped cleanly | `return ExitCode::SUCCESS` |
| `Err(EFSC_CONNECT / 1063)` | Not under SCM | `None` → fall through to CLI |
| `Err(_)` | Unexpected failure | `return ExitCode::FAILURE` |

On non-Windows the entire block is cfg-gated out; the binary continues
straight to CLI mode.

### Service lifecycle

```
SCM invokes ffi_service_main (extern "system")
    └─ service_main(args)
        └─ run_service(args)
            ├─ init_service_tracing (JSON to stderr; SCM captures it)
            ├─ register SCM control handler (sends on stop_tx on Stop)
            ├─ report ServiceState::Running
            ├─ tokio::runtime::Builder::new_multi_thread().build()
            │   └─ run_tasks(config, stop_rx)
            │       ├─ 15-min collect interval → svc_collect_and_enqueue
            │       ├─ 30-sec drain interval   → svc_drain
            │       └─ stop_rx.changed() → final drain (10-s timeout) → break
            └─ report ServiceState::Stopped
```

### Stop handling

The SCM control handler (a `Fn` closure) sends `true` on a
`tokio::sync::watch` channel when it receives `ServiceControl::Stop`.  The
async task loop selects on `stop_rx.changed()`, runs one last drain with a
10-second `tokio::time::timeout`, then breaks out of the loop.  The runtime
`.block_on(…)` call returns and `run_service` reports `Stopped` to the SCM.

### Unsafe code

`define_windows_service!` expands to an `extern "system"` FFI trampoline that
contains compiler-generated `unsafe`.  The rest of the crate uses
`#[deny(unsafe_code)]`; `service.rs` carries its own `#![allow(unsafe_code)]`
and the module declaration in `lib.rs` has `#[allow(unsafe_code)]`.

---

## Files changed

| File | Change |
|------|--------|
| `crates/agent/Cargo.toml` | `windows-service` dep bumped to `"0.8"` |
| `crates/agent/src/service.rs` | **NEW** — service dispatcher integration |
| `crates/agent/src/lib.rs` | Added `#[cfg(windows)] pub mod service` |
| `crates/agent/src/main.rs` | Try service mode first; `#![deny]` for unsafe |
| `crates/agent/tests/service_smoke.rs` | **NEW** — cfg-gated smoke tests |

---

## Verification

### Automated (any platform)

```sh
cargo check -p agent            # must pass on Linux / macOS (cfg-gating)
cargo test  -p agent            # service_smoke tests are no-ops on Linux
```

### Manual (Windows VM required)

See `docs/provisioning/04-windows-test-vm.md` for VM setup.

#### Register and start the service

```powershell
$bin = "C:\ProgramData\CMTraceOpen\cmtraceopen-agent.exe"
sc.exe create CMTraceOpenAgent binPath=$bin start=auto
sc.exe start  CMTraceOpenAgent
sc.exe query  CMTraceOpenAgent   # expect STATE: 4  RUNNING
```

#### Verify console fall-through

```cmd
cmtraceopen-agent.exe --oneshot
```

Should complete normally (not hang waiting for SCM).

#### Stop and inspect

```powershell
Stop-Service CMTraceOpenAgent
# Wait ~10 s for drain timeout
sc.exe query CMTraceOpenAgent    # expect STATE: 1  STOPPED

# Check stderr captured by SCM (requires event-log provider registration,
# deferred to a later wave — read the agent log file instead):
Get-Content "$env:ProgramData\CMTraceOpen\agent.log" -Tail 20
```

Expected log lines (JSON):

```jsonc
{"message":"service status set to Running", ...}
{"message":"entering service task loop", ...}
// ... periodic drain/collect lines ...
{"message":"SCM Stop received", ...}
{"message":"stop signal received; draining in-flight work", ...}
{"message":"final drain completed", ...}   // or "timed out"
{"message":"service task loop exited", ...}
{"message":"service status set to Stopped", ...}
```

---

## Deferred

- Pause/Continue control handling
- Event log provider registration (tracing to stderr is fine for v1)
- Restart-on-config-change
- MSI installer (separate issue: "Wave 4 impl: WiX MSI sources")
