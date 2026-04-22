# Wave 4 — Day-2 Operations Runbook

> **Audience:** the on-call operator (today: Adam). This is the
> bind-it-on-the-wall reference for what to do when the platform misbehaves.
> Optimized for being read at 3 a.m. on a phone — every section is
> self-contained, every action is imperative, and every metric / command
> resolves to a real surface in the shipped code.

> **Note:** Several thresholds in this document reference Prometheus
> metric names with the `cmtrace_*` prefix. The `/metrics` exposition
> endpoint itself is **not** shipped in `api-v0.1.0` (see
> [release notes — Known limitations](../release-notes/api-v0.1.0.md#known-limitations)).
> Where a `cmtrace_*` name appears below it is the **target name** to
> wire up alongside the existing per-route counters on `/` (see
> [`crates/api-server/src/routes/status.rs`](../../crates/api-server/src/routes/status.rs)).
> Until `/metrics` lands, the equivalent diagnostic is "scrape the
> JSON-formatted access log" or "eyeball `GET /` and `GET /v1/sessions`".
> Each section calls out the today-substitute explicitly.

---

## Table of contents

1. [Topology recap](#1-topology-recap)
2. [Routine ops (weekly)](#2-routine-ops-weekly)
3. [Incident playbooks](#3-incident-playbooks)
   - [A. All devices stopped checking in](#a-all-devices-stopped-checking-in)
   - [B. One device stopped checking in](#b-one-device-stopped-checking-in)
   - [C. Bundle ingest failure rate spike](#c-bundle-ingest-failure-rate-spike)
   - [D. api-server crashlooping](#d-api-server-crashlooping)
   - [E. Cert chain broke after Cloud PKI rotation](#e-cert-chain-broke-after-cloud-pki-rotation)
   - [F. Agent installed but never sends a bundle](#f-agent-installed-but-never-sends-a-bundle)
4. [Capacity planning thresholds](#4-capacity-planning-thresholds)
5. [Disaster recovery](#5-disaster-recovery)
6. [Observability stack](#6-observability-stack)
7. [Backup + retention policy](#7-backup--retention-policy)
8. [On-call expectations](#8-on-call-expectations)
9. [Post-mortem template](#9-post-mortem-template)

---

## 1. Topology recap

cmtraceopen runs three tiers: Windows endpoints carry the agent (a
service that collects evidence and ships it as a chunked bundle over
mTLS), the api-server (an Axum binary in a distroless container on
BigMac26 today, future K8s) accepts the bundle and parses it on ingest,
and the React viewer reads device / session / entries via the same
api-server. SQLite + local-FS blob store back the PoC; Postgres + Azure
Blob are the production targets. Device identity comes from a client
cert SAN URI (`device://{tenant}/{aad-device-id}`), with the cert chain
rooted at Gell CDW Workspace Labs Root → Issuing CAs (Intune Cloud PKI
auto-renews via SCEP).

```
  +----------------------------------+        +----------------------+
  | Windows endpoint                 |        | Operator browser     |
  |                                  |        |                      |
  |  cmtraceopen-agent (service)     |        |  Vite + React SPA    |
  |   + collectors (logs, evtx,      |        |   - Local (WASM)     |
  |     dsregcmd, evidence orch.)    |        |   - API mode         |
  |   + on-disk queue                |        |                      |
  |     %ProgramData%\               |        +----------+-----------+
  |       cmtraceopen-agent\queue\   |                   |
  |   + uploader (chunked, retries)  |          Bearer (Entra JWT)
  |                                  |          GET /v1/devices, sessions, entries
  |  Cert: LocalMachine\My           |                   |
  |   issued by Cloud PKI Issuing CA |                   v
  +-----------------+----------------+        +----------------------+
                    |                         | api-server :8080     |
       chunked bundle upload  ---mTLS-------> |  (Axum, distroless)  |
       /v1/ingest/bundles                     |                      |
       /v1/ingest/bundles/{id}/chunks         |  parse-on-ingest     |
       /v1/ingest/bundles/{id}/finalize       |  worker              |
                                              |                      |
                                              |  + SQLite (PoC)      |
                                              |  + blob_fs /data     |
                                              |    (or Postgres +    |
                                              |     Azure Blob, prod)|
                                              +----------+-----------+
                                                         |
                       Intune Cloud PKI ---SCEP---> Agent (cert renewal)
                                                         |
                       Gell CDW Workspace Labs Root + Issuing CA
                       (chain anchor on api-server's CMTRACE_CLIENT_CA_BUNDLE)
```

For the canonical diagram and protocol details see
[`docs/architecture.md`](../architecture.md). For the Cloud PKI chain
specifics see `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`
(local memory, not in repo).

---

## 2. Routine ops (weekly)

Block one hour on Monday morning. These five checks should be silent
when nothing is wrong; surface noise here is what becomes Sev-3 work.

### 2.1 Disk usage trend

1. SSH to the runner host (`ssh adam@192.168.2.50`).
2. Capture this week's blob footprint:
   ```bash
   du -sh /var/lib/cmtraceopen/data/blobs
   df -h /var/lib/cmtraceopen
   ```
3. Append the result to `~/cmtraceopen-cap.csv` (week, GB used, GB free).
4. Compare delta vs. the previous week.

| Week-over-week growth | Action |
| --- | --- |
| `< 10 GB / week` | Nothing. PoC noise. |
| `10–50 GB / week` | Note in weekly status; no action. |
| `> 50 GB / week sustained 2 wks` | Open a capacity-planning ticket. Plan migration to Azure Blob (`BlobStore` trait already in place; Azure impl pending — see `BlobStore` in [`crates/api-server/src/storage/mod.rs`](../../crates/api-server/src/storage/mod.rs)). |
| `> 80% of host disk` | **Sev 2.** Run [Playbook D § Disk-full branch](#d-api-server-crashlooping). |

> **Note:** Blob growth scales with `devices × bundles_per_day × bundle_size`.
> See [§4 Capacity planning](#4-capacity-planning-thresholds) for the
> per-device math.

### 2.2 Cert expiry survey

Cloud PKI auto-renews via SCEP, so this should be silent. Verify:

1. From any operator browser, hit the viewer's API mode and load
   `/v1/devices`. The list reflects the most recent cert per device
   (the `last_seen_utc` field is bumped on each ingest, which only
   succeeds with a valid cert post-Wave-3).
2. For any device whose `last_seen_utc` is `> 14 d` ago, suspect cert
   issuance failure. Pull its detail:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" \
     https://api.cmtraceopen.example/v1/devices/<id>
   ```
   The cert fingerprint + `notAfter` come from the SAN URI extraction
   that runs on every connection (see
   [`crates/api-server/src/auth/device_identity.rs`](../../crates/api-server/src/auth/device_identity.rs)).
3. In the Intune portal, Devices → search by `aad-device-id`. The
   "Certificates" tab shows current PKCS profile state. If renewal is
   stuck, retrigger from the device side:
   ```powershell
   # On the endpoint, as Admin
   Get-ScheduledTask -TaskName "*PKCS*" | Start-ScheduledTask
   ```
4. Cloud PKI portal → Certificates → Expiry < 30 d filter. If any
   device cert is < 30 d and Intune isn't auto-renewing, the issuance
   profile may be misconfigured; see Playbook E.

> **Note:** When Prometheus lands (see [§6](#6-observability-stack)), wire
> `cmtrace_device_cert_days_until_expiry{device_id}` as a gauge so this
> becomes a passive alert instead of a manual sweep.

### 2.3 Backup verification

1. Confirm the nightly backup job ran (cron entry on the runner host):
   ```bash
   ssh adam@192.168.2.50 'tail -50 /var/log/cmtraceopen-backup.log'
   ```
2. Verify the most recent dump exists and is non-zero:
   ```bash
   ls -lh /backup/cmtraceopen/$(date +%F)/
   ```
3. Once a quarter, restore the most recent backup into a scratch
   container and confirm the schema + a `SELECT count(*) FROM sessions;`
   round-trip works. Backup-you-haven't-restored is hope, not backup.

See [§7 Backup + retention policy](#7-backup--retention-policy) for
the rotation matrix.

### 2.4 CRL refresh sanity

1. Today's substitute (until `/metrics` ships): grep the api-server log
   for the CRL refresh tick:
   ```bash
   docker compose logs api-server --since=2h | grep -i "crl"
   ```
2. Expected: at least one "crl refresh ok" entry per hour.
3. When `/metrics` lands, the equivalent is:
   ```promql
   increase(cmtrace_crl_refresh_total[2h]) > 0
   ```
4. If silent for > 2 h, see Playbook E (CRL stall is a leading indicator
   of cert-chain trouble).

> **Note:** `api-v0.1.0` ships **without** CRL polling
> ([release notes — Known limitations](../release-notes/api-v0.1.0.md#known-limitations)).
> This check is forward-looking; until CRL polling lands, revocation is
> a manual "remove from `CMTRACE_CLIENT_CA_BUNDLE` + restart" workflow.

### 2.5 Dependency advisory check

The CI runs `cargo audit` weekly via the workflow added in PR #52.

1. In GitHub → Actions → "cargo-audit weekly" → most recent run.
2. Read the report. Any new findings?
   - **None / informational only:** done.
   - **Yellow advisory (no fix):** open a tracking issue, label `security`,
     watch upstream.
   - **Red advisory with fix available:** branch, bump, PR, merge. If
     the affected crate is in the runtime path (rustls, axum, sqlx,
     reqwest, aws-lc-rs), promote to Sev-2.
3. The same workflow also opens a PR if `Cargo.toml` and `Cargo.lock`
   ever drift (PR #52); review and merge those routinely.

> **Note:** Dependabot (PR #54) handles routine version bumps; treat
> those as ordinary drive-by reviews, not Day-2 work.

---

## 3. Incident playbooks

Each playbook follows the same shape:

1. **Signal** — what tells you this is happening
2. **First 5 minutes** — stop-the-bleeding
3. **Next 15 minutes** — diagnostics
4. **Likely root causes** — what to look for
5. **Fix patterns** — common remediations
6. **Post-mortem trigger** — when to write one (see [§9](#9-post-mortem-template))

---

### A. All devices stopped checking in

#### A.1 Signal

- **Primary (when shipped):** `rate(cmtrace_ingest_bundles_initiated_total[5m])`
  is `0` for `> 2 h` during business hours.
- **Today's substitute:** `GET /` status page → "Recent bundles"
  table — newest `ingestedUtc` is `> 2 h` old.
- **Secondary:** viewer's device list shows every device with
  `last_seen_utc > 2 h` ago.

> Severity: **Sev 1**. 4 h response.

#### A.2 First 5 minutes

1. Confirm the api-server is up:
   ```bash
   curl -k https://api.cmtraceopen.example/healthz
   # expect: {"status":"ok","service":"api-server","version":"0.1.0"}
   ```
2. If `/healthz` answers but ingest is silent, jump to **A.4 likely
   causes** with focus on cert chain (item 3) or network reachability.
3. If `/healthz` does **not** answer, jump to [Playbook D](#d-api-server-crashlooping).
4. Post a Sev-1 line in the incident channel (today: Slack DM to
   yourself / commit message scratch file): `cmtraceopen Sev-1 ingest
   stall, investigating, will update in 15`.

#### A.3 Next 15 minutes — diagnostics

1. Tail the server log for ingest-init failures:
   ```bash
   ssh adam@192.168.2.50 'docker compose logs api-server --tail=200 \
     | grep -iE "ingest|init|tls|handshake|cert"'
   ```
2. Test mTLS handshake from a fresh shell (not from a known-good agent):
   ```bash
   openssl s_client -connect api.cmtraceopen.example:443 -showcerts \
     -servername api.cmtraceopen.example </dev/null
   ```
   - Server cert must be valid (not expired, hostname match).
   - The chain shown should end at the configured root CA.
3. Test from an actual agent host (RDP to one of the test VMs):
   ```powershell
   # Try a manual ingest probe against /healthz
   Invoke-WebRequest -Uri https://api.cmtraceopen.example/healthz -UseBasicParsing
   ```
4. Cloud PKI portal → Issuing CAs → check for a rotation event in the
   last 24 h. A silent CA rotation breaks every device handshake at
   once and is the #1 cause of "all devices stopped" outside of
   server-down.
5. Check the runner's network egress / firewall change log; coincidence
   with a network maintenance window is the #2 cause.

#### A.4 Likely root causes

| Cause | Tell-tale signal |
| --- | --- |
| api-server crashed | `/healthz` doesn't answer → switch to Playbook D |
| Network outage to runner | `openssl s_client` connection refused / times out |
| Cert chain broke (CA rotation) | `openssl s_client` shows a cert chain not anchored in your bundle → Playbook E |
| TLS clock skew | Server log: `certificate is not yet valid` / `expired`. Run `chronyc tracking` (or equivalent) on host |
| Server ingest path returning 503 | `cmtrace_ingest_bundles_initiated_total` increments but `..._finalized_total{status="failed"}` spikes → switch to Playbook C |
| Hostile firewall change | Other services on the host also unreachable; coincidence with change-window |

#### A.5 Fix patterns

- **Server down:** see Playbook D.
- **Cert rotation:** see Playbook E.
- **Network outage:** open a network ticket; agents will auto-resume
  via the on-disk queue when reachability returns (see
  [`crates/agent/src/queue.rs`](../../crates/agent/src/queue.rs) — the
  retry/backoff loop survives reboots and partial uploads dedupe at
  the byte level on the server side).
- **Clock skew:** sync time on the runner host (`sudo chronyc -a
  makestep`), restart api-server.

#### A.6 Post-mortem trigger

Mandatory. Use [§9](#9-post-mortem-template).

---

### B. One device stopped checking in

#### B.1 Signal

- **Primary:** that device's `/v1/devices/{id}.last_seen_utc` is
  `> 48 h` ago, while peer devices are checking in normally.
- **Secondary:** an end user pings asking why their device isn't
  showing logs.

> Severity: **Sev 3** (single user impact). Best-effort.

#### B.2 First 5 minutes

1. Confirm the device record exists and grab its identity:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" \
     https://api.cmtraceopen.example/v1/devices/<id>
   ```
2. Check the corresponding device record in the Intune portal. If
   Intune shows the device as **inactive / unenrolled / wiped**, this
   isn't an incident — close as "device retired".
3. If Intune shows the device as healthy and online, proceed.

#### B.3 Next 15 minutes — diagnostics

1. RDP / SSH to the endpoint.
2. Check the agent service status:
   ```powershell
   Get-Service CMTraceOpenAgent
   # Status should be: Running
   ```
3. Check whether the cert is present and valid:
   ```powershell
   Get-ChildItem Cert:\LocalMachine\My |
     Where-Object { $_.Subject -like '*cmtraceopen*' -or $_.Issuer -like '*Workspace Labs*' } |
     Format-Table Subject, NotAfter, Thumbprint
   ```
4. Confirm Entra device join is healthy (the SAN URI depends on the
   AAD device id):
   ```powershell
   dsregcmd /status | Select-String -Pattern 'AzureAdJoined|DeviceId|TenantId'
   ```
5. Inspect the agent's queue and logs:
   ```powershell
   Get-ChildItem $env:ProgramData\cmtraceopen-agent\queue\
   Get-Content $env:ProgramData\cmtraceopen-agent\logs\agent.log -Tail 100
   ```
6. Check whether bundles are stuck in `Failed` state with retries
   exhausted:
   ```powershell
   Get-ChildItem $env:ProgramData\cmtraceopen-agent\queue\*.json |
     ForEach-Object { Get-Content $_ | ConvertFrom-Json } |
     Where-Object { $_.state.status -eq 'Failed' } |
     Select-Object @{N='id';E={$_.metadata.bundleId}}, @{N='attempts';E={$_.state.attempts}}, @{N='lastError';E={$_.state.lastError}}
   ```

#### B.4 Likely root causes

| Cause | Tell-tale signal |
| --- | --- |
| Device offline (vacation, broken laptop) | Intune shows last check-in days ago |
| Cert revoked but agent still trying | Sidecar logs show repeated `403`/`401` |
| Intune unenrolled the device | `dsregcmd /status` → `AzureAdJoined: NO` |
| Agent service stopped + auto-restart exhausted | `Get-Service` returns `Stopped`; Event Viewer → Application log shows crash dumps |
| Queue dir permissions broke | Sidecar logs show `Access is denied` writing to `%ProgramData%\cmtraceopen-agent\queue\` |
| Endpoint endpoint URL wrong (config drift) | `config.toml` has stale hostname after migration |

#### B.5 Fix patterns

- **Device offline:** wait. The agent's queue persists across reboots
  and the uploader resumes from the server-recorded byte offset (see
  retry policy in
  [`crates/agent/src/uploader.rs`](../../crates/agent/src/uploader.rs#L36-L58)).
- **Cert revoked / unenrolled:** treat as decommission. Remove the
  device record (admin route — see
  [`crates/api-server/src/routes/admin.rs`](../../crates/api-server/src/routes/admin.rs)).
- **Service stopped:** `Start-Service CMTraceOpenAgent`. If it
  immediately stops, capture the event log entry, file an issue, and
  consider rolling the device back to the previous agent MSI version
  via Intune (see [§5.3](#53-agent-rolled-back-to-a-buggy-version)).
- **Permissions:** reset ACLs:
  ```powershell
  icacls "$env:ProgramData\cmtraceopen-agent" /reset /T
  ```
- **Config drift:** redeploy the MSI from Intune (will rewrite
  `config.toml`).

#### B.6 Post-mortem trigger

Skip unless this is the third single-device incident in 30 days
(suggests an agent quality problem; promote to Sev-2).

---

### C. Bundle ingest failure rate spike

#### C.1 Signal

- **Primary:** `rate(cmtrace_ingest_bundles_finalized_total{status="failed"}[5m]) > 0.05`
  (5 % of finalizes failing).
- **Today's substitute:** sample failed sessions:
  ```bash
  curl -H "Authorization: Bearer $TOKEN" \
    "https://api.cmtraceopen.example/v1/sessions?parse_state=failed&limit=10"
  ```
  More than ~3 entries with recent `ingested_utc` is the trigger.
- **Secondary:** server log shows recurring `parse worker failed` or
  `blob write failed` lines.

> Severity: **Sev 2** (degraded ingest). 24 h response, but escalate
> to Sev 1 if rate exceeds 25 %.

#### C.2 First 5 minutes

1. Distinguish ingest failure from parse failure — they have different
   fix paths:
   ```bash
   # When metrics are wired:
   #   ingest failures:  cmtrace_ingest_bundles_finalized_total{status="failed"}
   #   parse failures:   cmtrace_parse_worker_runs_total{result="failed"}
   ```
   Today, distinguish via `parse_state` on the session row:
   - `parse_state = "failed"` → parse-side problem
   - `parse_state = "ok"` but bundle never appeared → ingest-side
2. Disk full? Quick check:
   ```bash
   ssh adam@192.168.2.50 'df -h /var/lib/cmtraceopen'
   ```
3. If disk > 95 % full, jump to [Playbook D § Disk-full branch](#d4-likely-root-causes).

#### C.3 Next 15 minutes — diagnostics

1. Pull the most recent failed sessions and look for a pattern:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" \
     "https://api.cmtraceopen.example/v1/sessions?parse_state=failed&limit=10"
   ```
   Are they all from the same device? Same agent version? Same
   `content_kind`?
2. Sample one failed bundle's full lifecycle in logs:
   ```bash
   docker compose logs api-server --since=1h \
     | grep -i "<session_id_or_device_id>" \
     | head -100
   ```
3. Check parse worker liveness:
   ```bash
   docker compose logs api-server --since=15m | grep -i "parse worker"
   ```
   Expected: at least one "parse worker run" log per minute under load.
   Total silence = worker has crashed.
4. Check blob store writes:
   ```bash
   docker compose logs api-server --since=15m | grep -iE "blob|finalize|sha"
   ```
   Look for `EACCES` / `permission denied` / `no space left`.

#### C.4 Likely root causes

| Cause | Tell-tale signal | Fix |
| --- | --- | --- |
| Disk full on api-server | `df` > 95 %; logs say `no space left` | Free space (purge old blobs per [§7](#7-backup--retention-policy)), restart server |
| Blob store permission regression | Logs say `EACCES` writing `/data/blobs/...` | `chown -R 65532:65532 /data` (the distroless container runs as `nonroot`); restart |
| Parse worker crashed | "parse worker" log silent; ingest still finalizes but `parse_state` stays `pending` | Restart api-server (worker is in-process); if reproducible, capture the failing fixture and file a parser regression |
| Malformed bundles from a buggy agent build | All failed sessions share the same agent version | Pin Intune to the previous good MSI (see [§5.3](#53-agent-rolled-back-to-a-buggy-version)); investigate the regression |
| SQLite contention under load | Logs say `database is locked`; `pool_size` (status page) saturated | Verify WAL is on (it is by default — see release notes); consider Postgres migration if recurrent |

#### C.5 Fix patterns

- **Disk full:** see Playbook D.
- **Worker crash:** restart container. If the failing bundle is
  reproducible, copy it out of `/data/blobs/...` and add it to
  `tools/fixtures/` as a regression test.
- **Bad agent build:** roll back the Intune MSI assignment to the
  previous version. Devices uninstall + reinstall on next sync (~ 8 h).

#### C.6 Post-mortem trigger

Required if rate exceeded 5 % for > 1 h, or if a parse regression
made it past CI.

---

### D. api-server crashlooping

#### D.1 Signal

- **Primary:** container restarts > 5 in 1 h (`docker compose ps` shows
  a recent uptime + bumped restart count).
- **Secondary:** `/healthz` unreachable; viewer shows 502/503; ingest
  metrics flat-line.

> Severity: **Sev 1**. 4 h response.

#### D.2 First 5 minutes

1. Snapshot the container state:
   ```bash
   ssh adam@192.168.2.50 'docker compose ps'
   ```
2. Capture recent logs **before** restarting (if you restart first
   you'll lose the panic backtrace):
   ```bash
   ssh adam@192.168.2.50 'docker compose logs api-server --tail=500' \
     > /tmp/cmtraceopen-crash-$(date +%s).log
   ```
3. Check host memory + disk pressure:
   ```bash
   ssh adam@192.168.2.50 'free -m && df -h /var/lib/cmtraceopen'
   ```

#### D.3 Next 15 minutes — diagnostics

1. Look for a panic backtrace:
   ```bash
   grep -iE "panic|backtrace|fatal|aborted" /tmp/cmtraceopen-crash-*.log | head -30
   ```
2. Check for OOM:
   ```bash
   ssh adam@192.168.2.50 'sudo dmesg | grep -iE "oom|killed process" | tail -20'
   ```
3. Check for port conflict (the `:8080` collision pattern):
   ```bash
   ssh adam@192.168.2.50 'sudo lsof -i :8080'
   ```
4. Try restarting with verbose logging:
   ```bash
   ssh adam@192.168.2.50 \
     'cd /srv/cmtraceopen && RUST_LOG=trace docker compose up api-server'
   ```
   Watch the first 30 s — most startup failures (bad env var, missing
   cert file, malformed CA bundle, migration failure) emit a clear
   error before the process exits.

#### D.4 Likely root causes

| Cause | Tell-tale signal | Fix |
| --- | --- | --- |
| Disk full (writes failing) | `df` near 100 %; logs say `no space left on device` | Free blob space (per [§7](#7-backup--retention-policy)); restart; if recurring, accelerate Azure Blob migration |
| OOM | `dmesg` shows `Killed process` for `api-server`; container's last exit was 137 | Bump container memory limit in `docker-compose.yml`; profile to find the leak (`cargo flamegraph`) |
| Panic in parse worker | Backtrace mentions `cmtraceopen-parser` | Pin to last-known-good submodule pointer; capture the offending bundle as a fixture |
| Port conflict | `lsof :8080` shows another process | Kill the squatter or change the api-server's `CMTRACE_LISTEN_ADDR` |
| DB corruption (SQLite WAL torn) | Logs say `database disk image is malformed` on startup | **Restore from backup** (see [§5.1](#51-server-lost-bigmac-dies)); do not run `PRAGMA integrity_check` against the live file |
| Migration failure on startup | Logs reference `migrations/` and an SQL error | Roll back to the previous image tag; investigate the migration locally |

#### D.5 Fix patterns

- **Buy time first, fix root cause second.** If the immediate fix is
  "restart and it stays up", do that, then dig into the cause without
  the on-call pressure.
- **Image pin / rollback:** the runner's `docker-compose.yml` references
  `ghcr.io/adamgell/cmtraceopen-api:latest` by default. Pin to the
  previous tag while investigating:
  ```yaml
  image: ghcr.io/adamgell/cmtraceopen-api:0.1.0
  ```
  then `docker compose pull && docker compose up -d`.
- **Hot reload of env / certs:** the api-server has no SIGHUP handler
  today; cert + env changes require a container restart.

#### D.6 Post-mortem trigger

Mandatory. Use [§9](#9-post-mortem-template).

---

### E. Cert chain broke after Cloud PKI rotation

#### E.1 Signal

- **Primary:** every new device handshake fails with TLS verification
  error; api-server logs show repeated
  `unknown ca` / `certificate verify failed` / `bad certificate`.
- **Secondary:** [Playbook A](#a-all-devices-stopped-checking-in) signal
  (no ingests) **plus** Cloud PKI portal shows a recent CA rotation.

> Severity: **Sev 1**. 4 h response. Will affect 100 % of fleet.

#### E.2 First 5 minutes

1. Confirm the rotation happened — Cloud PKI portal → Issuing CAs →
   check the "Issued" / "NotBefore" timestamps for a recent CA.
2. Confirm the server's CA bundle does not yet include the new issuer:
   ```bash
   ssh adam@192.168.2.50 \
     'openssl crl2pkcs7 -nocrl -certfile $CMTRACE_CLIENT_CA_BUNDLE \
      | openssl pkcs7 -print_certs -noout \
      | grep -i "subject="'
   ```
   Compare the subjects against the Cloud PKI portal's current
   issuing CA list.
3. If a mismatch is confirmed, this is your root cause.

#### E.3 Next 15 minutes — fix

1. Export the new issuing CA (and root, if rotated) from the Cloud PKI
   portal as PEM.
2. **Use the dual-CA bundle pattern** for rotation-with-overlap so
   in-flight devices keep working while the new cert propagates:
   ```bash
   cat /etc/cmtraceopen/ca-old.pem /etc/cmtraceopen/ca-new.pem \
     > /etc/cmtraceopen/ca-bundle.pem
   ```
   The bundle is a concatenation; rustls walks the list and accepts a
   chain anchored at any included root.
3. Reload:
   ```bash
   ssh adam@192.168.2.50 'cd /srv/cmtraceopen && docker compose restart api-server'
   ```
   (No hot-reload today — see [§D.5](#d5-fix-patterns).)
4. Verify mTLS handshake works again from a known-good agent.
5. Schedule removal of `ca-old.pem` for **after** every device has
   rotated to a cert chained to `ca-new.pem`. Cloud PKI's renewal
   window is typically 24–48 h; pad to 7 days, then drop the old
   anchor.

#### E.4 Likely root causes

| Cause | Tell-tale signal |
| --- | --- |
| Cloud PKI rotated issuing CA, server bundle not updated | Subjects in `CMTRACE_CLIENT_CA_BUNDLE` don't match Cloud PKI portal |
| Root CA rotated (rare) | Same as above but at the root layer |
| `CMTRACE_CLIENT_CA_BUNDLE` env var pointed at a stale path after a host migration | File path doesn't exist or contains old certs |
| Rustls + aws-lc-rs verifier rejecting a valid chain | Unlikely; would emit a specific aws-lc-rs error in logs. File a bug. |

#### E.5 Fix patterns

- **Dual-CA bundle for rotation overlap** (canonical):
  ```bash
  # /etc/cmtraceopen/ca-bundle.pem
  -----BEGIN CERTIFICATE-----
  ...ca-old issuing CA...
  -----END CERTIFICATE-----
  -----BEGIN CERTIFICATE-----
  ...ca-new issuing CA...
  -----END CERTIFICATE-----
  ```
  Restart, verify, drop the old after rotation completes.
- **Emergency single-CA swap:** if Cloud PKI did a hard cutover (no
  overlap), accept temporary outage, swap the bundle, restart. Devices
  will reconnect within minutes once their renewed certs are in place.

#### E.6 Post-mortem trigger

Mandatory. Capture the rotation timeline (when did Cloud PKI rotate?
when did we notice? when did we deploy the new bundle?). Add an alert
for "Cloud PKI rotation detected but server bundle unchanged for > 1 h"
to the Wave 5 backlog.

---

### F. Agent installed but never sends a bundle

#### F.1 Signal

- **Primary:** Intune reports MSI install **success** for a target device,
  but the device never appears in `/v1/devices`.
- **Secondary:** end-user / fleet-owner report — "I deployed the agent
  to 50 devices, only 32 showed up".

> Severity: **Sev 2** (initial onboarding broken). 24 h response.

#### F.2 First 5 minutes

1. RDP to one of the affected devices.
2. Confirm the service is installed and running:
   ```powershell
   Get-Service CMTraceOpenAgent
   ```
3. If `Status: Stopped`, try to start it manually:
   ```powershell
   Start-Service CMTraceOpenAgent
   Get-EventLog -LogName Application -Source 'CMTraceOpenAgent' -Newest 10
   ```

#### F.3 Next 15 minutes — diagnostics

1. **Cert presence**:
   ```powershell
   Get-ChildItem Cert:\LocalMachine\My |
     Where-Object { $_.Issuer -like '*Workspace Labs*' } |
     Format-Table Subject, Thumbprint, NotAfter
   ```
   Empty? The Intune PKCS profile hasn't issued yet (race with first
   sync). Wait one Intune cycle (~ 8 h) or force a sync from
   Settings → Accounts → Access work or school → Info → Sync.
2. **Endpoint config**:
   ```powershell
   Get-Content $env:ProgramData\cmtraceopen-agent\config.toml
   ```
   Verify the `endpoint` field matches the production api-server FQDN.
   This is baked into the MSI at build time; a dev MSI in a prod
   target group is a common cause.
3. **Queue contents**:
   ```powershell
   Get-ChildItem $env:ProgramData\cmtraceopen-agent\queue\
   ```
   - Empty: collectors haven't produced any bundle yet (or fired and
     failed before enqueue).
   - Has `.zip` + `.json` pairs: bundles are queued but the uploader
     can't deliver them. Inspect the sidecar:
     ```powershell
     Get-Content $env:ProgramData\cmtraceopen-agent\queue\*.json
     ```
     The `state` field will be `Pending`, `Failed { lastError }`, or
     `Done` (see
     [`crates/agent/src/queue.rs`](../../crates/agent/src/queue.rs#L38-L57)).
4. **Logs**:
   ```powershell
   Get-Content $env:ProgramData\cmtraceopen-agent\logs\agent.log -Tail 200
   ```
5. **Network reachability**:
   ```powershell
   Test-NetConnection api.cmtraceopen.example -Port 443
   ```
   - Failure: device firewall, corporate proxy, or split-DNS issue.

#### F.4 Likely root causes

| Cause | Tell-tale signal | Fix |
| --- | --- | --- |
| Cert not yet present (Intune sync race) | `Cert:\LocalMachine\My` empty for cmtraceopen | Wait or force Intune sync |
| Wrong endpoint in `config.toml` | Endpoint points at dev / stale FQDN | Reissue MSI from correct ring; uninstall + reinstall via Intune |
| Queue dir permissions | Sidecar logs say `Access is denied` writing queue | `icacls "$env:ProgramData\cmtraceopen-agent" /reset /T` |
| Network firewall blocks 443 outbound | `Test-NetConnection` fails | Open firewall ticket; agent will resume on its own once clear |
| Service installed but not started | `Get-Service` returns `Stopped` | `Start-Service`; if it stops again immediately, check Application event log |
| MSI install succeeded but service registration failed (rare WiX bug) | No `CMTraceOpenAgent` service exists despite MSI success | Uninstall + reinstall. File a bug if reproducible. |

#### F.5 Fix patterns

- **Most cases self-heal:** the agent's queue persists, the uploader
  retries with backoff, and the on-disk state survives reboots. If you
  fix the root cause (cert appears, network opens, etc.) the device
  catches up automatically.
- **Reissuance pattern for misconfig:** uninstall via Intune
  (Apps → Properties → Assignments → Uninstall), wait for sync,
  reassign the correct MSI build.

#### F.6 Post-mortem trigger

Required if > 10 % of an MSI deployment ring fails to onboard within
24 h.

---

## 4. Capacity planning thresholds

### 4.1 Per-tier ceilings

| Tier | PoC ceiling | Migration trigger | Migration target |
| --- | --- | --- | --- |
| **Devices** | ~100 (SQLite comfortable) | > 250 devices | Postgres (already wired into compose for Wave 4) |
| **Bundles / day** | ~400 / day (100 devices × 4 bundles) ~50 k entries / day — SQLite fine | > 10 k bundles / day | Shard by device or move to Postgres + parse worker pool |
| **Blob storage** | ~5 MB / bundle uncompressed; 100 dev × 4 bundles × 30 d ≈ **60 GB / month** | > 100 GB sustained | Azure Blob (`BlobStore` trait already abstracts this — Azure impl pending) |
| **Network in** | 100 dev × 5 MB × 4 bundles / day ≈ **2 GB / day** | > 50 GB / day sustained | Move api-server off home-LAN; CDN-front the viewer |

### 4.2 SQLite → Postgres migration cues

Move when **any** of the following is true for two consecutive weeks:

- Device count > 250
- Status page's `pool_size` is saturated (max_size reached) under load
- `database is locked` warnings appear in logs more than once a day
- Blob throughput requires concurrent ingest writers (current SQLite
  setup serializes finalize commits)

The `MetadataStore` trait
([`crates/api-server/src/storage/mod.rs`](../../crates/api-server/src/storage/mod.rs))
already abstracts the metadata layer; the Postgres implementation is
the next slot. The compose stack runs Postgres + Adminer side-by-side
today specifically so this migration is a backend swap, not a
re-architecture.

### 4.3 Sizing math (one-glance reference)

```
storage_per_month_GB  = devices × bundles_per_day × 5 MB × 30 d / 1024
network_per_day_GB    = devices × bundles_per_day × 5 MB / 1024
entries_per_day       = devices × bundles_per_day × ~125 entries/bundle
```

Plug in your fleet size before any capacity meeting.

---

## 5. Disaster recovery

Three named scenarios. Each has a recovery time objective (RTO) and
recovery point objective (RPO) baked into the procedure.

### 5.1 Server lost (BigMac dies)

**RTO:** 4 h. **RPO:** 24 h (last nightly Postgres dump).

1. Provision a replacement host (or restore the BigMac26 VM image, see
   `dev/bigmac-runner-kit/`).
2. Restore Postgres from the most recent dump:
   ```bash
   psql -h newhost -U cmtraceopen -d cmtraceopen \
     < /backup/cmtraceopen/$(date +%F)/cmtraceopen.sql
   ```
3. Restore the blob store via rsync:
   ```bash
   rsync -avz --delete \
     /backup/cmtraceopen/$(date +%F)/blobs/ \
     newhost:/var/lib/cmtraceopen/data/blobs/
   ```
4. Bring up the stack:
   ```bash
   ssh newhost 'cd /srv/cmtraceopen && docker compose up -d'
   ```
5. Verify:
   ```bash
   curl -k https://newhost/healthz
   curl -k https://newhost/readyz
   ```
6. **In-flight bundle recovery is automatic.** The agent's resumable
   uploader (see
   [`crates/agent/src/uploader.rs`](../../crates/agent/src/uploader.rs))
   re-inits any partial uploads — the server tells the client the
   authoritative `resume_offset` and the agent picks up from there.
   Bundles fully queued client-side but never delivered will retry on
   the next agent tick (see RetryPolicy: 3 attempts at 1s/5s/30s, then
   the on-disk queue's exponential backoff via `mark_failed`).

### 5.2 Cloud PKI tenant lost

**RTO:** 24 h. **RPO:** N/A — re-issue, no data loss.

1. Re-provision Cloud PKI per
   `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md` (the
   reference contains the actual root + issuing CA configuration
   already provisioned for Wave 3).
2. Reissue device certs through Intune. Auto-renewal will hand out new
   certs on the next sync (~ 8 h per device, 24 h fleet-wide).
3. Update `CMTRACE_CLIENT_CA_BUNDLE` on the api-server to the new
   issuing CA(s), restart.
4. **During the rotation window**, leave the **old** CA in the bundle
   too (dual-CA pattern from Playbook E) so devices already issued
   from the old CA can continue ingesting until their renewal lands.
5. Once Intune confirms 100 % renewal, drop the old CA from the bundle
   and restart.

### 5.3 Agent rolled back to a buggy version

**RTO:** 8 h (Intune sync cycle). **RPO:** N/A — agent state is
rebuildable on the device.

1. In Intune → Apps → cmtraceopen-agent → Properties → Assignments,
   change the target group's assignment from "current MSI" to
   "previous good MSI".
2. Devices uninstall the bad version and reinstall the previous good
   version on next sync (~ 8 h).
3. **Queue contents survive an MSI uninstall** because the queue lives
   in `%ProgramData%\cmtraceopen-agent\queue\`, not
   `%ProgramFiles%`. Verify the WiX uninstaller does not rm-rf
   `%ProgramData%` (current behavior: leaves it alone — see the WiX
   script in the agent crate). If queue files are wiped, in-flight
   bundles are lost (RPO becomes "the next collection cycle").
4. Open a bug for the regression, pin the bad version out of the
   release pipeline.

---

## 6. Observability stack

### 6.1 Currently shipping (`api-v0.1.0`)

| Surface | Endpoint / location | What it gives you |
| --- | --- | --- |
| Liveness | `GET /healthz` | `{"status":"ok",...}` — process is alive |
| Readiness | `GET /readyz` | Identical to `/healthz` today; will probe deps once Postgres is wired |
| Status page | `GET /` | Uptime, total request count, top-8 routes by count, SQLite pool stats, recent 10 bundles |
| Per-route counters | `GET /` (rendered) | Counter map keyed by `MatchedPath`; `unmatched` bucket for true 404s |
| Structured logs | `stderr` (collected by docker compose) | JSON-lines tracing output, filter via `RUST_LOG` |
| Recent ingests | `GET /` table | last 10 sessions: device_id, session_id (8-char prefix), parse_state, ingestedUtc |

See [`crates/api-server/src/routes/status.rs`](../../crates/api-server/src/routes/status.rs)
for the implementation.

### 6.2 Recommended for the prod beta

> **Note:** The release notes explicitly call out
> "[No `/metrics`](../release-notes/api-v0.1.0.md#known-limitations)" as
> a current limitation. This section is the recommended Wave 4–5
> investment.

1. **Prometheus + Grafana sidecar.** Compose-able onto BigMac26 today;
   already enough disk + memory headroom. Scrape config:
   ```yaml
   scrape_configs:
     - job_name: cmtraceopen-api
       scrape_interval: 30s
       static_configs:
         - targets: ['api-server:8080']
       metrics_path: /metrics
   ```
2. **`/metrics` endpoint on api-server.** Adopt the
   `metrics-exporter-prometheus` crate (no `ring` dep — verify with
   `cargo tree -p api-server | grep ring` per the contributor guide).
3. **Target metric names** (use `cmtrace_*` prefix consistently):
   - `cmtrace_ingest_bundles_initiated_total{device_id, content_kind}` — counter
   - `cmtrace_ingest_bundles_finalized_total{status="ok"|"failed", content_kind}` — counter
   - `cmtrace_ingest_chunks_received_total{status}` — counter
   - `cmtrace_parse_worker_runs_total{result="ok"|"failed"}` — counter
   - `cmtrace_parse_worker_queue_depth` — gauge
   - `cmtrace_crl_refresh_total{result}` — counter
   - `cmtrace_device_cert_days_until_expiry{device_id}` — gauge
   - `cmtrace_http_request_duration_seconds{route, method, status}` — histogram
4. **Initial alert set** (Slack webhook receiver, single channel):

   | Alert | Condition | Severity |
   | --- | --- | --- |
   | `BundleIngestFailureRate` | `rate(cmtrace_ingest_bundles_finalized_total{status="failed"}[5m]) > 0.05` for 10 m | Sev 2 |
   | `IngestStall` | `rate(cmtrace_ingest_bundles_initiated_total[15m]) == 0` for 2 h (business hours only) | Sev 1 |
   | `ParseWorkerStall` | `cmtrace_parse_worker_queue_depth > 100` for 10 m | Sev 2 |
   | `HealthzDown` | `up{job="cmtraceopen-api"} == 0` for 2 m | Sev 1 |

5. **Log shipping.** Today's `docker compose logs` is fine; for prod
   beta, ship to Loki (compose-able with Grafana) so logs are
   queryable alongside metrics.

---

## 7. Backup + retention policy

### 7.1 Postgres metadata

| Backup | Cadence | Retention | Location |
| --- | --- | --- | --- |
| Full dump (`pg_dump -Fc`) | Weekly (Sunday 02:00) | 30 days | `/backup/cmtraceopen/<YYYY-MM-DD>/cmtraceopen.dump` |
| WAL archive | Continuous | 14 days | `/backup/cmtraceopen/wal/` |

Restore drill: monthly. Mount the most recent full + WAL into a
scratch container, confirm `SELECT count(*) FROM sessions;` matches
expectations.

### 7.2 Blob store

| Backup | Cadence | Retention | Location |
| --- | --- | --- | --- |
| `rsync` of `/data/blobs` | Daily (03:00) | 30 days | `/backup/cmtraceopen/<YYYY-MM-DD>/blobs/` |

Storage retention for the **live** blob store:

- **Default (today):** 90 days. Older blobs are eligible for purge.
- **Configuration:** intended env var is `CMTRACE_BUNDLE_TTL_DAYS`.

> **Note:** `CMTRACE_BUNDLE_TTL_DAYS` does **not exist yet** in the
> shipped api-server. Ship-blocker for any environment with regulatory
> retention requirements. Track as Wave 4 follow-up; until then,
> implement retention via an out-of-band cron + `find -mtime`:
> ```bash
> find /var/lib/cmtraceopen/data/blobs -type f -mtime +90 -delete
> ```
> (Coordinate with metadata: a blob purged out from under the metadata
> store will surface as 404s in the viewer. Safer is a script that
> joins against `sessions` and only deletes orphans + age-past
> entries.)

### 7.3 Server-side structured logs

| Source | Retention | Notes |
| --- | --- | --- |
| `docker compose logs` (json-file driver) | 14 days, rotated | Set `max-size: 50m, max-file: 14` in compose's `logging:` block |
| Future Loki ingest | 30 days | Once shipped per [§6.2](#62-recommended-for-the-prod-beta) |

---

## 8. On-call expectations

### 8.1 Coverage

Single on-call rotation today: the project owner (Adam). When a second
operator joins, switch to PagerDuty (or equivalent) with a 1-week
rotation; until then, the on-call is implicit and async-friendly
(every signal in this runbook can be diagnosed from a laptop without
specialist tools).

### 8.2 Severity definitions

| Severity | Definition | Response SLA | Examples |
| --- | --- | --- | --- |
| **Sev 1** | Total outage. No devices ingesting; viewer unusable; cert chain broken globally. | **4 h** acknowledge + active mitigation | Playbooks A, D, E |
| **Sev 2** | Degraded service. > 10 % failure rate, single device cluster down, MSI deployment ring failing. | **24 h** acknowledge | Playbooks C, F |
| **Sev 3** | Cosmetic / single-device. Status page glitch, single agent quirk, individual user impact. | Best-effort | Playbook B |

### 8.3 Communication

- **Internal status:** post a single line per incident in the project
  channel (today: a scratch `INCIDENTS.md` in the home dir; later: a
  Slack `#cmtraceopen-incidents` channel).
- **External (end users):** for Sev 1 / Sev 2, send a status note to
  affected device owners within 1 h of detection. Template:
  > "We're seeing degraded log collection from cmtraceopen since
  > `<UTC time>`. We're working on it. No action needed on your end —
  > queued logs will deliver automatically once service is restored."
- **Post-incident:** use [§9](#9-post-mortem-template).

### 8.4 Escalation

There is no escalation path today (single operator). When the team
grows, add an escalation rotation here. Until then, the action when
stuck is: stop, sleep, retry — most cmtraceopen incidents degrade
gracefully because the agent queues client-side and the server resumes
ingest when restored.

---

## 9. Post-mortem template

Open a post-mortem doc at `docs/postmortems/<YYYY-MM-DD>-<slug>.md`
within 5 business days of any Sev-1 or Sev-2 incident. Use this
template:

```markdown
# Post-mortem: <short title>

- **Date / time (UTC):** YYYY-MM-DD HH:MM
- **Duration:** Xh Ym
- **Severity:** Sev <1 | 2 | 3>
- **Author:** <name>
- **Status:** draft | reviewed | actions-tracked

## TL;DR

One paragraph. What broke, who noticed, how we fixed it, what we'll do
to prevent recurrence.

## Impact

- Devices affected: <count or fleet %>
- Bundles delayed: <approx>
- Bundles lost: <count, or "none — all queued + delivered after fix">
- User-visible symptoms: <bullets>

## Timeline (UTC)

- HH:MM — first signal (what tripped it)
- HH:MM — on-call paged / noticed
- HH:MM — diagnostic step X
- HH:MM — root cause identified
- HH:MM — mitigation applied
- HH:MM — service confirmed restored
- HH:MM — incident closed

## Root cause

Plain-language explanation. Link to the playbook that applied (or note
if none did — that's a doc gap).

## What went well

- ...

## What went poorly

- ...

## Action items

| Item | Owner | Priority | Tracked in |
| --- | --- | --- | --- |
| ... | adam | P1 | issue #NNN |

## Lessons learned

Free-form. Optional but encouraged.
```

---

_Last updated: 2026-04-21. When you bring a new tool / surface online
(e.g. shipping `/metrics`, swapping SQLite → Postgres, wiring CRL
polling), revisit this runbook and replace the "today's substitute"
notes with the real metric names. The runbook is the operations bible
— keep it accurate or it stops being trusted._
