# Backlog — macOS agent

**Filed:** 2026-04-30
**Discovered during:** Path A validation of heartbeat + on-demand bundles on a Mac dev box
**Status:** open, no committed delivery date

## Why this is on the backlog

During Path A validation, we ran the `cmtraceopen-agent` 0.2.0 binary on macOS to dogfood the heartbeat + on-demand-bundle flow before signing the Windows MSI. The WS path, heartbeat persistence, request_bundle handling, and ingest correlation all worked end-to-end. The on-demand bundle that came back, however, was only **336 bytes** and parsed as `parse_state = "partial"` — because every existing collector targets Windows-only paths:

- `LogsCollector` walks paths like `C:\Windows\CCM\Logs\**`
- `EventLogsCollector` reads Windows Event Log via the `windows` crate (Win32 EVTX)
- `DsRegCmdCollector` shells out to `dsregcmd.exe`
- `AgentLogsCollector` reads `%ProgramData%\CMTraceOpen\Agent\logs`

On macOS each of those returns zero files, the orchestrator zips an empty staging dir, finalize succeeds, the parser sees zero log files → `partial`. The lifecycle works; the bundle is empty.

If we want macOS to be a first-class supported platform — for dev dogfooding, MDM coverage of mixed fleets, or even just better integration tests — we need macOS-native collectors.

## Goals (if/when this gets prioritized)

1. The same 0.2.0 agent binary builds for `aarch64-apple-darwin` and `x86_64-apple-darwin` and ships meaningful evidence on demand.
2. A bundle from a macOS device contains:
   - `system.log` / `unified-log` excerpts (last N hours, configurable via `log_paths`-equivalent)
   - MDM enrollment state (parallel to dsregcmd: `profiles -P` or Jamf framework status)
   - Agent's own self-log
3. The api-server's parsers either accept the new file shapes or classify them gracefully (probably extend the parser registry — separate question).
4. CI agent-msi workflow gains a sibling `agent-pkg.yml` that builds + signs + notarizes a `.pkg` for distribution via Jamf or Intune for Mac.

## Sketch of the work

### Collectors (`crates/agent/src/collectors/`)

Each existing collector struct is a concrete type. Path of least surgery: add a `cfg(target_os = "macos")` block per collector with a macOS-specific impl, OR introduce a trait-object indirection so the orchestrator picks the right one at construction time. The latter is cleaner long-term.

| Existing | macOS analog | Source |
|---|---|---|
| `LogsCollector` (CCM/Intune log paths) | macOS unified log slice | `log show --predicate ... --last 4h --style ndjson` |
| `EventLogsCollector` (Windows EVTX) | `system.log` + `wifi.log` + `install.log` excerpts | direct file read from `/var/log` |
| `DsRegCmdCollector` (Azure AD join state) | MDM state + Apple Business Manager enrollment | `profiles -P` (root) + `/Library/Managed Preferences/` snapshot |
| `AgentLogsCollector` (agent self-log) | already platform-agnostic — point at `/Library/Application Support/CMTraceOpen/Agent/logs` instead | trivial path swap |

### Service host

Today's `service.rs` is Windows SCM. macOS analog is **launchd**:
- Ship a `LaunchDaemon` plist (lives at `/Library/LaunchDaemons/com.cmtraceopen.agent.plist`).
- Daemon binary path matches the current binary location convention.
- The agent's `main.rs` already falls through to CLI/daemon mode when SCM-connect fails on Windows — a `cfg(target_os = "macos")` branch can skip the SCM probe entirely and jump straight to the same `run_cli`-equivalent.

### Build + signing pipeline

A new GHA workflow `.github/workflows/agent-pkg.yml`:
- Trigger on `agent-v*` tags (same trigger as the MSI workflow — both build artifacts in parallel).
- Self-hosted Mac runner OR macos-latest GHA hosted runner with notarization secrets. (Notarization needs an Apple Developer account + an app-specific password OR notarization profile.)
- Build `cargo build --release --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`, lipo into a universal binary.
- Sign with Developer ID Application cert.
- Wrap in a `.pkg` via `pkgbuild` + `productbuild`.
- Notarize with `xcrun notarytool submit --wait`, staple with `xcrun stapler staple`.
- Attach to the same GitHub Release the MSI is attached to.

### Distribution

If the goal is mixed fleet management:
- **Intune for Mac:** supports `.pkg` distribution out of the box. Same Win32-app-equivalent package upload, reachable from the same admin console.
- **Jamf:** the canonical Mac MDM. `.pkg` uploaded to JCDS, installer policy targets a Smart Group.

The **`reconfigure-cmtrace-agent.ps1` equivalent** is a `bash` script that writes `/Library/Application Support/CMTraceOpen/Agent/config.toml` and reloads the LaunchDaemon. A `tools/jamf-deploy/` directory paralleling `tools/ninjaone-deploy/` is the natural home.

## Risks / unknowns

1. **`log show` performance** — the macOS unified log can be huge and slow to query; need to bound the time window aggressively (default 1 h, configurable).
2. **`dsregcmd` parity** — the `profiles -P` output is XML, not the structured text the existing parser expects. Either add a macOS-specific parser kind or normalize the output during collection.
3. **Code-signing cert** — current Cloud PKI flow is Windows-only; Apple Developer signing is a different cert + workflow + cost. Budget question.
4. **Notarization latency** — Apple's notary service can take minutes (rare hours). Tag-to-release time becomes asymmetric vs the MSI path.
5. **Dev convenience vs production scope** — if this is purely for dev dogfooding, an unsigned local `cargo build` is enough and we don't need `.pkg` / notarization / Jamf at all. Decide upfront which use case is being served.

## Acceptance criteria (when prioritized)

- A macOS dev can `cargo run -p agent` against a local api-server, receive a request_bundle frame, and finalize a bundle that the parser categorizes as `ok` or `ok-with-fallbacks` — not `partial` due to empty content.
- A signed + notarized `.pkg` is produced by CI on `agent-v*` tags.
- A `reconfigure-cmtrace-agent.sh` script exists alongside the existing `.ps1` and does the same job for Mac fleets.
- At least one collector specifically tests on a macOS GHA runner (CI gate).

## Pointers

- `crates/agent/src/collectors/mod.rs` — the trait-and-orchestrator surface.
- `crates/agent/src/collectors/evidence.rs` — `EvidenceOrchestrator::new` constructor, currently Windows-shaped.
- `crates/agent/src/runtime.rs::build_components` — where the orchestrator gets wired in.
- `.github/workflows/agent-msi.yml` — the existing Windows workflow to mirror for macOS.
- `tools/ninjaone-deploy/` — distribution scripts to mirror.

## Decision log

- 2026-04-30: filed. Path A validation works without it; deferred until Path B (Windows fleet rollout) is healthy on real devices and we have signal on whether mixed-fleet support is worth the macOS pipeline cost.
