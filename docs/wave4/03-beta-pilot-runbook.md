# 03 — Beta Pilot Runbook (14-day, 8-device, real Entra tenant)

Operational runbook for a **time-boxed, 14-calendar-day beta** of the
`cmtraceopen` agent + api-server stack against a real Entra tenant and
5–10 real Windows devices. The goal is to graduate the walking skeleton
from "works on the test VM" (per
[`docs/provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md))
to "works on a small fleet of human-owned endpoints", with explicit
success criteria, daily observability, and a one-command rollback.

> **Note:** this runbook **presumes** the api-server is reachable from
> every pilot device. As of writing, the only deployed instance lives on
> `BigMac26` at `192.168.2.50:8080` — a private LAN address. Off-corp
> pilot devices **cannot reach it** until either (a) the api-server is
> redeployed to a publicly routable, TLS-terminated endpoint, or (b) a
> Cloudflare Tunnel is fronted onto BigMac26. See
> [Section 4](#section-4--api-server-prep-internet-reachability-is-the-gating-issue)
> below — this is the single biggest beta blocker.

---

## Series context

- **Wave 4** ships the agent as a signed MSI deployed via Intune. The
  authoritative project memo is `~/.claude/projects/F--Repo/memory/project_wave4_msi_intune_deploy.md`.
- This is doc 03 in the `docs/wave4/` series. Doc 01 is the MSI build
  recipe, doc 02 is the Intune-Graph deploy script (in flight as PR #57
  at the time of writing), and this doc takes the deployable agent and
  walks it onto real devices.

Read first:

1. [`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)
   — Cloud PKI, the PKCS profile, and how the SAN URI feeds the
   api-server's identity layer.
2. [`docs/provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md)
   — the single-device, hand-rolled version of what this runbook scales
   to 8 devices.
3. `docs/wave4/02-intune-graph-deploy.md` (PR #57, in flight) — the
   `Deploy-CmtraceAgent.ps1` script this runbook invokes.
4. [`docs/release-notes/api-v0.1.0.md`](../release-notes/api-v0.1.0.md)
   — current API capabilities and the **Known limitations** section
   (CRL polling not wired, no `/metrics` until PR #48 lands, etc.).

---

## Phase summary

| Phase | Window | Goal |
| --- | --- | --- |
| 0 — Pre-flight | Day −7 → Day 0 | Tenant prep, device selection, comms, api-server prep, backups, success metrics locked |
| 1 — Day 0 deployment | 90-min change window | Push the MSI + cert profile, watch first check-ins, confirm first bundle per device |
| 2 — Daily monitoring | Day 1 → Day 7 | 5-min daily checks; escalate on offline / failure-rate triggers |
| 3 — Wrap | Day 14 | Generate beta report; decide GA / extend / rollback |

---

## Success metrics (locked at Day −7, scored at Day 14)

These five numbers are the only thing that matters at Day 14. If any
fails, the beta does **not** graduate to GA without an explicit waiver.

| # | Metric | Target | How measured |
| --- | --- | --- | --- |
| 1 | **Adoption** | ≥ 7 / 8 devices visible in `GET /v1/devices` within 24 h of MSI assignment | API query at Day 1 09:00 |
| 2 | **Liveness** | ≥ 7 / 8 devices submit ≥ 1 finalized bundle within 7 days | API query at Day 7 09:00 |
| 3 | **Reliability** | Bundle ingest success rate ≥ 99 % across all retries | `cmtrace_ingest_bundles_finalized_total{status="ok"}` ÷ `…_total{status=~"ok\|failed"}` |
| 4 | **Footprint** | Agent CPU < 0.5 % time-averaged over 24 h on each device; `%ProgramData%\CMTraceOpen\` < 500 MB sustained | Per-device `Get-Counter` sample on Day 7 + 14; `Get-ChildItem` size sweep |
| 5 | **No user-visible regressions** | Zero pilot-user complaints traceable to the agent | Pilot inbox + escalation log |

> **Note:** metrics 3 and 4 lean on the Prometheus `/metrics` endpoint
> from PR #48 and on per-device PowerShell sampling. If PR #48 hasn't
> merged by Day 0, fall back to api-server log grep for failed
> finalize spans (`ingest_finalize.*status=failed`) — same numerator,
> uglier denominator.

---

## Phase 0 — Pre-flight (Day −7 → Day 0)

### Section 1 — Tenant prep

1. Confirm Cloud PKI is provisioned per
   [`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md).
   Check the "Done" criteria checklist at the bottom of that doc — every
   box must be ticked.
2. Create (or reuse) a `pilot` Entra security group:
   - Entra portal → **Groups** → **New group**.
   - Type: **Security**, name: `cmtraceopen-pilot`, membership:
     **Assigned**.
3. Assign the existing PKCS certificate profile (from runbook 03 step 4)
   to `cmtraceopen-pilot` **in addition to** `cmtraceopen-testdevices`.
   Profile assignment is additive — the test VM keeps its cert.
4. Confirm Intune **MDM auto-enrollment** is on: Entra → **Mobility (MDM
   and MAM)** → **Microsoft Intune** → **MDM user scope = All** (or a
   group containing every pilot user).
5. Verify Conditional Access does not block the agent's outbound HTTPS
   to the api-server:

   ```powershell
   # From any tenant-joined machine, confirm there is no CA policy that
   # blocks unmanaged-network egress for the pilot users.
   # Entra → Protection → Conditional Access → Policies →
   #   filter "Users: cmtraceopen-pilot"  →  expect: no blocking policy.
   ```

   The agent runs as `LocalSystem` and does not present a user identity
   on its egress, so user-scoped CA is almost never an issue — but
   device-scoped CA (e.g. "block from non-compliant device") would
   bite. Verify and document.

### Section 2 — Pilot device selection criteria

Target: **8 devices**. Below 5, the signal is too noisy to draw
conclusions from. Above 12, the operator can't triage a "just one
device" bug across the fleet without sinking the whole 14 days into
single-device debugging.

The 8 must span the matrix below; each row of the matrix must have at
least one device.

| Axis | Target spread |
| --- | --- |
| **OS version** | Mix of Windows 11 23H2 and 24H2 |
| **Form factor** | At least 1 physical laptop and 1 VM |
| **Network posture** | At least 1 corp LAN, 1 always-VPN, 1 off-corp residential |
| **User role** | At least 1 sysadmin, 1 developer, 1 standard user (validates per-user perms don't break LocalSystem collection) |
| **Geography** | At least 1 US-east + 1 US-west endpoint (surfaces clock-skew issues at the edges) |

Capture the actual matrix in a CSV before Day 0:

```csv
device_id,user,os_version,form_factor,network,role,geo
PILOT-01,alice,11-23H2,laptop,corp-lan,sysadmin,us-east
PILOT-02,bob,11-24H2,laptop,vpn,developer,us-east
PILOT-03,carol,11-23H2,vm,off-corp,standard,us-east
PILOT-04,dave,11-24H2,laptop,off-corp,standard,us-west
PILOT-05,erin,11-23H2,laptop,corp-lan,developer,us-west
PILOT-06,frank,11-24H2,laptop,vpn,sysadmin,us-east
PILOT-07,grace,11-23H2,vm,corp-lan,standard,us-west
PILOT-08,henry,11-24H2,laptop,off-corp,developer,us-west
```

Save as `dev/pilots/wave4-pilot-roster.csv` (gitignored — contains
real usernames). The CSV is the single source of truth for who's in
the pilot.

> **Note:** the standard-user row is load-bearing. The agent runs as
> `LocalSystem` so it should ignore per-user ACLs, but the only way to
> *prove* that is to put it on a box where the signed-in user has zero
> admin rights and confirm `%ProgramData%\CMTraceOpen\` still fills up.

### Section 3 — Pilot user comms

Send the announcement template (Appendix B.1) **on Day −7**, with the
go-live date called out as Day 0. The agent is silent — no UI, no
balloon notifications, no tray icon. So "feels off" can only mean CPU
or disk weirdness, which is unlikely. The comms must:

1. Name the operator-on-call (`Adam Gell`, acgell995@gmail.com) as the
   single escalation point.
2. State the rollback ETA (**< 30 min from go signal**) so users know
   the eject button is real.
3. Include the explicit symptom-to-watch-for: sudden machine slowdown,
   fan spin-up while idle, or `%ProgramData%\CMTraceOpen\` exceeding
   500 MB.
4. Tell users the agent runs as a Windows service named
   `CMTraceOpenAgent` so they can verify it themselves via `services.msc`.

### Section 4 — api-server prep (internet-reachability is the gating issue)

The api-server today lives on `BigMac26` at `192.168.2.50:8080`. That
address is **not routable from the public internet**, so any pilot
device that's off-corp and not on a routable VPN cannot ship bundles.

Pick **one** of the three options before Day 0:

#### Option A — Azure Container Apps + Application Gateway (recommended)

1. Build & push the api-server image to ACR:

   ```bash
   az acr login --name <acr-name>
   docker pull ghcr.io/adamgell/cmtraceopen-api:0.1.0
   docker tag  ghcr.io/adamgell/cmtraceopen-api:0.1.0 <acr-name>.azurecr.io/cmtraceopen-api:0.1.0
   docker push <acr-name>.azurecr.io/cmtraceopen-api:0.1.0
   ```

2. Provision a Container Apps environment + the api-server app with the
   env contract from [api-v0.1.0 release notes](../release-notes/api-v0.1.0.md)
   (Configuration → TLS / mTLS section). Mount the Cloud PKI trust
   bundle (`gell-pki-root.pem`) as a secret volume at
   `/etc/cmtraceopen/certs/client-ca.pem`.

3. Front the Container App with **Application Gateway v2** with mTLS
   termination — App Gateway terminates the device cert and re-presents
   it to the Container App via the configured backend cert. Server
   cert: a real public TLS cert (use Azure-issued from Key Vault or
   Let's Encrypt via cert-manager), **not** the Cloud PKI cert.

4. Wire the public hostname (`pilot.cmtraceopen.com` or whatever you
   provision) and the Cloud PKI trust bundle as
   `CMTRACE_CLIENT_CA_BUNDLE`.

5. Snapshot the Postgres-backed metastore + blob volume (Section 5)
   before Day 0.

#### Option B — Cloudflare Tunnel from BigMac26 (lazy alternative)

1. Install `cloudflared` on BigMac26.
2. `cloudflared tunnel create cmtraceopen-pilot`.
3. Route a hostname (`pilot.cmtraceopen.com`) through the tunnel to
   `http://192.168.2.50:8080`.
4. Cloudflare terminates TLS at the edge with a public cert. **mTLS
   client-cert termination is more painful through CF Tunnel** — you'd
   need CF Access mTLS rules, which can do it but require more wiring.
   For the beta window, accept that Option B sacrifices end-to-end
   client-cert verification at the api-server and falls back to the
   `X-Device-Id` header path (`CMTRACE_MTLS_REQUIRE_INGEST=false`).
   Document this as a known weakness of Option B.

#### Option C — keep BigMac26, restrict pilot to corp-LAN devices only

If A and B are both impractical for the beta window, restrict the pilot
device matrix to devices that can reach `192.168.2.50:8080` via corp
LAN or VPN. This **violates the network-posture spread** in Section 2
and weakens the beta — call this out in the Day 14 wrap report.

#### The single config knob

Whichever option you pick, **one** agent config field changes per
device — `api_endpoint` in `%ProgramData%\CMTraceOpen\Agent\config.toml`.
The MSI install line bakes it in:

```powershell
msiexec /i CMTraceOpenAgent.msi `
  CMTRACE_API_ENDPOINT=https://pilot.cmtraceopen.com `
  /qn
```

### Section 5 — Backup the api-server

Before going live, snapshot:

1. **Postgres** (when on Container Apps with managed Postgres) or
   **SQLite + blob volume** (when on BigMac26).

   ```bash
   # BigMac26 path:
   ssh bigmac26 'cd /srv/cmtraceopen && \
     docker compose exec -T postgres pg_dump -U cmtrace cmtrace \
     > /srv/cmtraceopen/backups/pre-pilot-$(date +%F).sql && \
     tar czf /srv/cmtraceopen/backups/blobs-pre-pilot-$(date +%F).tgz ./data/blobs'
   ```

2. Restore drill — verify the backup actually round-trips. **Do this on
   Day −3, not Day 0.** If restore fails, you have time to fix.

   ```bash
   # On a scratch host:
   docker compose down && rm -rf ./data
   psql ... < pre-pilot-YYYY-MM-DD.sql
   tar xzf blobs-pre-pilot-YYYY-MM-DD.tgz -C .
   docker compose up -d
   curl -sf http://scratch:8080/healthz
   ```

3. Target **restore wall-clock < 15 min** end-to-end. If the actual
   drill blows that budget, fix before Day 0 — the rollback playbook
   (Appendix C) assumes <15 min.

### Section 6 — Define escalation triggers

Locked at Day −7, executed during Phase 2:

| Trigger | Action |
| --- | --- |
| 1 device offline > 48 h | Email the user (template B.2), ask for a reboot |
| 2+ devices offline > 48 h | Halt new bundles (`docker compose stop api-server`), investigate root cause before resuming |
| Bundle failure rate spikes > 5 % over any rolling 1 h window | Halt, capture api-server logs + agent logs from one affected device, triage |
| Any pilot user reports performance complaint | Pull that device, root-cause within 24 h, document |
| Any cert revocation event | Investigate within 1 h (Cloud PKI portal) — should be zero during the beta |

---

## Phase 1 — Day 0 deployment (90-min change window)

Operator script. All steps numbered, all commands copy-pasteable.

1. **Pre-deploy checklist.** Walk every Phase 0 section and confirm
   "Done". Don't proceed past any unchecked item.
2. **Snapshot the api-server** (Section 5 commands, run again — the
   freshest snapshot wins as the rollback target).
3. **Dry-run the deploy.** Run the Graph-automation script with the
   `-DryRun` flag (lands with PR #57):

   ```powershell
   .\dev\intune-graph-deploy\Deploy-CmtraceAgent.ps1 `
     -DeviceGroupName "cmtraceopen-pilot" `
     -ApiEndpoint "https://pilot.cmtraceopen.com" `
     -DryRun
   ```

   Expect: a printed plan (assignment intent, target device count = 8,
   intunewin manifest hash) and **no Graph mutations**. If the printed
   target count ≠ 8, stop and reconcile the group membership before
   proceeding.

4. **Run for real.**

   ```powershell
   .\dev\intune-graph-deploy\Deploy-CmtraceAgent.ps1 `
     -DeviceGroupName "cmtraceopen-pilot" `
     -ApiEndpoint "https://pilot.cmtraceopen.com"
   ```

   The script:
   - Uploads the signed `.intunewin` to Intune as a Win32 LOB app.
   - Assigns the app to `cmtraceopen-pilot` as **Required**.
   - Confirms the PKCS cert profile is also assigned to the group.
   - Polls `/deviceManagement/managedDevices` for assignment status.

5. **Watch the api-server logs** for the first device check-in:

   ```bash
   # If on BigMac26:
   ssh bigmac26 'cd /srv/cmtraceopen && docker compose logs -f api-server'

   # If on Container Apps:
   az containerapp logs show -g rg-cmtraceopen-pilot \
     -n api-server --follow
   ```

   **Expected wall-clock:** median time-to-first-checkin from MSI
   assignment is **15–45 min** (Intune sync interval is 8 h by default
   but devices on the corp LAN typically sync within 30 min after
   `dsregcmd /refreshprt` or a reboot).

6. **As each device shows up, eyeball the bundle.** From any operator
   workstation:

   ```bash
   # Replace <DEVICE_ID> with the AAD device GUID from /v1/devices.
   curl -s "https://pilot.cmtraceopen.com/v1/devices/<DEVICE_ID>/sessions" \
     -H "Authorization: Bearer $OPERATOR_TOKEN" | jq .

   curl -s "https://pilot.cmtraceopen.com/v1/sessions/<SESSION_ID>/files" \
     -H "Authorization: Bearer $OPERATOR_TOKEN" | jq .
   ```

   You should see real CCM logs and friends. **Halt and triage** if any
   device's first bundle is empty or has zero parsed entries — that
   means collection ran but the parser didn't find anything, which is
   almost always a config / path bug.

7. **Send pilot user comms confirming go-live** (template B.3).

### Day 0 acceptance gate

Before declaring Day 0 a success, all of these must be true:

- [ ] The Graph deploy script exited 0.
- [ ] At least 4 of 8 devices have shown up in `GET /v1/devices` by end
      of the change window (the rest will catch up via Intune sync over
      the next 24 h).
- [ ] Every device that has shown up has at least one **non-empty**
      bundle parsed.
- [ ] api-server CPU on the host < 30 % steady-state.
- [ ] No 5xx in the api-server log during the change window.

If any of these fails, **invoke rollback** (Appendix C) and reschedule
Day 0 — do not push through partial success and let it bleed into Phase 2.

---

## Phase 2 — Day 1 → Day 7 monitoring

5-minute daily check, every morning.

### Daily checks (run between 08:00 and 09:00)

1. **Adoption count** — should be ≥ 7 by end of Day 1, ≥ 8 by end of Day 3:

   ```bash
   curl -s "https://pilot.cmtraceopen.com/v1/devices?limit=100" \
     -H "Authorization: Bearer $OPERATOR_TOKEN" \
     | jq 'length'
   ```

2. **Failed-finalize count** (PR #48):

   ```bash
   curl -s "https://pilot.cmtraceopen.com/metrics" \
     | grep '^cmtrace_ingest_bundles_finalized_total{status="failed"}'
   ```

   Expect a value of `0` or a small constant that does not grow
   day-over-day. Increment between yesterday and today should be ≤ 1.

3. **Failed parse-worker runs** (PR #48):

   ```bash
   curl -s "https://pilot.cmtraceopen.com/metrics" \
     | grep '^cmtrace_parse_worker_runs_total{result="failed"}'
   ```

   Same expectation as above.

4. **Server disk-usage trend** (blob store):

   ```bash
   ssh bigmac26 'du -sh /srv/cmtraceopen/data/blobs'
   ```

   Track day-over-day. ~10–50 MB/day/device is the expected order of
   magnitude. A device suddenly contributing > 500 MB/day is almost
   always a stuck collection loop and warrants pulling that one.

5. **Spot-check 3 random devices** — pull the most recent session for
   each and confirm:
   - `dsregcmd_status.txt` (or whichever evidence file the parser
     surfaces as the dsregcmd snapshot) has timestamps within the last
     24 h.
   - The file list looks complete relative to the [walking-skeleton
     baseline](https://github.com/adamgell/cmtraceopen-web/blob/main/README.md):
     CCM logs, Panther, CBS, evtx exports if enabled.

### Escalation triggers (live)

Re-run the Section 6 trigger table every morning. Any tripped trigger
takes priority over normal monitoring.

### Day 7 mid-pilot check

At Day 7 09:00, evaluate metric #2 (Liveness) explicitly. If < 7 / 8
devices have submitted a bundle, root-cause the laggards before Day 8.
Common causes ranked by frequency:

1. Device hasn't synced Intune since Day 0 — ask user to reboot.
2. PKCS cert profile failed silently on that device — re-run from
   [`docs/provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md)
   Section 7.
3. Network egress blocked at corporate firewall (less likely off-corp).

---

## Phase 3 — Day 14 wrap

### Section 7 — Generate the beta report

A single Markdown file at `dev/pilots/wave4-pilot-report.md`. Sections:

1. **Success metrics — actuals vs targets.** A copy of the Section
   "Success metrics" table with an `Actual` column filled in.
2. **Surprise incidents.** Anything that triggered an escalation,
   chronological. For each: timestamp, what tripped, what we did, ETA
   to fix.
3. **Resource ceiling.** Peak `cmtrace_db_connections_in_use`, peak
   server CPU + memory, peak agent CPU + memory across the fleet.
4. **Volume.** Total bundles ingested, total bytes stored, average
   bundle size, parse-success rate.
5. **Operator complaints.** Verbatim quotes from pilot users (with
   names redacted in any external-facing version).
6. **GA recommendation.** Pick one — see decision matrix below.

### Section 8 — GA / extend / rollback decision

| All 5 metrics met? | Unrecoverable incidents? | Decision |
| --- | --- | --- |
| Yes | None | **GA** — graduate to Wave 5 broad rollout |
| Yes | 1+ recoverable | **Extend pilot** by 7 days; re-evaluate |
| No (1–2 missed by < 10 %) | None | **Extend pilot** by 7 days, narrow scope to address gaps |
| No (3+ missed, or any missed by > 10 %) | Any | **Rollback** (Appendix C); root-cause; restart Phase 0 |

### Section 9 — If rolling back

1. Run the uninstall:

   ```powershell
   .\dev\intune-graph-deploy\Deploy-CmtraceAgent.ps1 `
     -DeviceGroupName "cmtraceopen-pilot" `
     -Uninstall
   ```

   This reassigns the Intune Win32 app from **Required** to
   **Uninstall**, which Intune executes on next sync (median 4 h).
2. Revoke the per-device certs in the Intune Cloud PKI portal:
   **Tenant administration** → **Cloud PKI** → issuing CA →
   **Certificates** → filter by serial / subject → **Revoke** with
   reason `cessationOfOperation`.
3. Stop the (cloud-deployed) api-server and re-snapshot for forensics:

   ```bash
   az containerapp update -g rg-cmtraceopen-pilot -n api-server --min-replicas 0 --max-replicas 0
   ```

4. Send the debrief comms (template B.4).
5. Run the [full rollback playbook](#appendix-c--rollback-playbook).

---

## Appendix A — Triage cheat sheet (top 5 expected failure modes)

### A.1 — Cert not present on device

**Symptom:** device never appears in `/v1/devices`; agent log shows
`mTLS handshake failed: client did not present a certificate` (when on
mTLS) or device hits the `X-Device-Id` fallback (when off mTLS).

**Diagnose on the device:**

```powershell
Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.Issuer -like '*Gell*' } |
  Select-Object Thumbprint, Subject, NotAfter
```

Empty result → PKCS profile didn't deliver. Check Intune admin center
→ device → Device configuration → PKCS profile status. If `Pending`,
force a sync; if `Error`, read the per-setting status for the why.

### A.2 — Network can't reach api-server

**Symptom:** agent log shows `connect: connection refused` or `dial
tcp: lookup pilot.cmtraceopen.com: no such host`.

**Diagnose on the device:**

```powershell
Test-NetConnection pilot.cmtraceopen.com -Port 443
Resolve-DnsName pilot.cmtraceopen.com
```

`TcpTestSucceeded : False` → corporate firewall, split-tunnel VPN, or
the api-server is down. Check api-server `/healthz` from another host
first to disambiguate.

### A.3 — Time skew

**Symptom:** mTLS handshake fails with `certificate not yet valid` or
`certificate has expired`; bundles ingest but their timestamps are
hours off.

**Diagnose on the device:**

```powershell
w32tm /query /status
w32tm /resync /force
```

Time off by > 5 min from UTC → resync. Persistent skew (e.g. on a VM
whose host clock drifts) → fix the host or set
`HKLM\SYSTEM\CurrentControlSet\Services\w32time\Config\MaxAllowedPhaseOffset`
appropriately.

> **Deep dive:** see
> [`docs/wave4/21-agent-network-time.md` — §1 Time Sync](./21-agent-network-time.md#1-time-sync)
> for full verification steps, failure modes, and VM-specific
> remediation guidance.

### A.4 — MSI install failed silently because no admin rights

**Symptom:** Intune reports the Win32 app as `Failed` for that device;
`%ProgramFiles%\CMTraceOpen\Agent\agent.exe` does not exist.

**Diagnose on the device:**

```powershell
Get-WinEvent -LogName Application -MaxEvents 50 |
  Where-Object { $_.ProviderName -eq 'MsiInstaller' } |
  Format-List TimeCreated, Message
```

Look for `1721` (an MSI installer error) or `1033` (install completed
with status `1603`, the canonical "fatal MSI error"). Most common root
cause: Intune is supposed to install Win32 apps under SYSTEM context,
which has admin — if it didn't, the MSI assignment was misconfigured
in Section 4. Verify `Install behavior = System` on the Win32 app.

### A.5 — Agent crashed

**Symptom:** device showed up once, then went silent. `services.msc`
shows `CMTraceOpenAgent` as `Stopped`.

**Diagnose on the device:**

```powershell
Get-WinEvent -LogName System -MaxEvents 50 |
  Where-Object { $_.Message -like '*CMTraceOpenAgent*' } |
  Format-List TimeCreated, Message

Get-Content "$env:ProgramData\CMTraceOpen\Agent\logs\agent.log" -Tail 200
```

Look for the panic line and capture it. Restart the service
(`sc.exe start CMTraceOpenAgent`); if it crashes again immediately,
file a bug with the panic snippet and pull that device from the
pilot via Intune (move it to a "rolled-back" group) until the bug
is fixed.

---

## Appendix B — Comms templates

### B.1 — Pilot announcement (sent Day −7)

> Subject: You're in the cmtraceopen pilot — go-live <YYYY-MM-DD>
>
> Hi <name>,
>
> Your device has been selected for a 14-day pilot of cmtraceopen, our
> internal tool for collecting and centralizing Windows management
> logs. On <YYYY-MM-DD> at <HH:MM>, Intune will install a small
> background service called `CMTraceOpenAgent` on your machine.
>
> **What you'll see:** nothing. The agent has no UI and no
> notifications. It runs as a Windows service, you can confirm it's
> there via `services.msc`.
>
> **What to watch for:** sudden machine slowdown, fan spinning while
> idle, or the folder `C:\ProgramData\CMTraceOpen\` exceeding 500 MB.
>
> **If anything feels wrong:** email Adam Gell
> (acgell995@gmail.com) — I will pull your device from the pilot
> within 30 minutes.
>
> The pilot ends on <YYYY-MM-DD + 14d>. After that we'll either roll
> out broadly or roll back; either way I'll let you know.
>
> Thanks for being a guinea pig.
>
> — Adam

### B.2 — Day-7 check-in (sent only if a device is offline > 48 h)

> Subject: Quick favor — please reboot your machine
>
> Hi <name>,
>
> Your machine hasn't checked in to the cmtraceopen api-server for
> ~48 h. Most often this is a stale Intune sync. Could you reboot
> your machine when convenient today?
>
> No urgency — if it's not back by tomorrow morning I'll dig in.
>
> — Adam

### B.3 — Go-live confirmation (sent end of Day 0)

> Subject: cmtraceopen pilot is live
>
> Hi all,
>
> The pilot is live as of today. <N>/8 devices have already checked
> in; the remaining devices will catch up via Intune sync over the
> next 24 h. No action needed from any of you.
>
> Status, escalation, and rollback timelines are unchanged from the
> Day −7 announcement.
>
> — Adam

### B.4 — Beta-end debrief (sent Day 14)

> Subject: cmtraceopen pilot wrap — <Decision: GA / Extended /
> Rolled back>
>
> Hi all,
>
> The 14-day cmtraceopen pilot ended today. Decision: <decision>.
> <One sentence per metric: what target was, what we hit.>
> <One sentence on incidents.>
>
> If GA: nothing changes for you — the agent stays installed.
> If extended: pilot continues for another 7 days; same comms cadence.
> If rolled back: Intune will uninstall the agent on next sync
> (within 4 h). You'll see no other change.
>
> Thanks for the patience. Detailed report is at
> `dev/pilots/wave4-pilot-report.md` for those who want it.
>
> — Adam

---

## Appendix C — Rollback playbook

Wall-clock target: **< 30 min from go signal to all-pilot-devices
agent-stopped**.

| Step | Command | Expected wall-clock |
| --- | --- | --- |
| 1 | Operator says "rollback" out loud + posts in #cmtraceopen-pilot | 0 min |
| 2 | `Deploy-CmtraceAgent.ps1 -Uninstall -DeviceGroupName "cmtraceopen-pilot"` | 1 min |
| 3 | Stop the api-server (Container Apps `--min-replicas 0` or `docker compose stop api-server` on BigMac26) | 1 min |
| 4 | Revoke the 8 pilot certs in the Cloud PKI portal | 5 min |
| 5 | Send debrief comms (template B.4) | 5 min |
| 6 | Verify uninstall on devices: `sc.exe query CMTraceOpenAgent` returns `1060: service does not exist` | 4 h (Intune sync) |
| 7 | If forensic data needed: keep the Postgres + blob backup from Section 5; do **not** restore over a fresh deploy | — |

Total wall-clock to "agent stops shipping": **~12 min**. Total to
"agent removed from device": one Intune sync interval (≤ 4 h on the
corp LAN, longer off-corp).

---

## "Done" criteria for this runbook

- [ ] Every Phase 0 section is checked off.
- [ ] Day 0 acceptance gate (Phase 1) passed.
- [ ] All 7 daily checks ran on Days 1–7 with no escalation triggers
      tripped (or every tripped trigger has a documented resolution).
- [ ] Day 14 beta report exists at `dev/pilots/wave4-pilot-report.md`.
- [ ] A GA / extend / rollback decision is recorded with the operator's
      sign-off.

---

## References

- [`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)
- [`docs/provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md)
- [`docs/wave4/21-agent-network-time.md`](./21-agent-network-time.md) — time sync + network policy deep-dive
- [`docs/release-notes/api-v0.1.0.md`](../release-notes/api-v0.1.0.md)
- Intune Win32 app deploy:
  <https://learn.microsoft.com/mem/intune/apps/apps-win32-app-management>
- Cloud PKI revoke a cert:
  <https://learn.microsoft.com/mem/intune/protect/cloud-pki-revoke-cert>
- Application Gateway mTLS:
  <https://learn.microsoft.com/azure/application-gateway/mutual-authentication-overview>
- Cloudflare Tunnel:
  <https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/>
- Prometheus scrape config (PR #48):
  see `docs/architecture.md` → "/metrics endpoint" once PR #48 merges.
