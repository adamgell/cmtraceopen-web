# Install the cmtraceopen agent on a new Windows box

Short runbook for turning a fresh Windows 11 / Server 2022 machine into a
cmtraceopen agent that reports into the api-server.

This is the **minimum path** — just agent, no GitHub runner. For the full
test-VM runbook with more context see
[`docs/provisioning/04-windows-test-vm.md`](../../docs/provisioning/04-windows-test-vm.md).

Prereq recap (already provisioned in the tenant):

- `cmtraceopen-testdevices` Entra group — devices get the Cloud PKI
  client-auth cert automatically.
- `Gell - SCEP Cert` profile — delivers the cert with the SAN URI the
  api-server's mTLS identity parser expects.
- `Gell - Root Trusted Cert` + issuing-CA trust profiles — chain validates
  on the device.

---

## Phase 0 — What you need

- The signed MSI from CI. Either:
  - the latest green `agent-msi.yml` run's artifact, e.g.
    `gh run download <run-id>` on your Mac and copy the `.msi` to the box, or
  - the release asset attached to an `agent-v*` tag once we cut one.
- Your api-server's URL **as reachable from the Windows VM**. Docker-compose
  on your Mac publishes `8080` — the VM needs either:
  - the Mac's LAN IP (same network): `http://<mac-ip>:8080`, or
  - a tunnel (tailscale, ngrok, ssh `-R`) pointing at the Mac's `8080`.
  - Quick LAN reachability check from the VM: `Test-NetConnection <mac-ip> -Port 8080`.
- The tenant group IDs you'll need for member-adds (from the provisioner output):

  ```
  cmtraceopen-testdevices : 533f1ce3-1ac6-4c37-9d02-5938faf2cf03
  ```

---

## Phase 1 — Windows install + Entra join

1. Install Windows 11 Pro/Enterprise or Windows Server 2022 with TPM 2.0 +
   Secure Boot (vTPM on a hypervisor is fine).
2. Keep the default hostname — Entra tracks the device by its AAD device ID,
   not by Windows hostname.
3. Settings → Accounts → **Access work or school** → **Connect** → **Join
   this device to Microsoft Entra ID** (NOT the "sign in" button). Sign in
   with a tenant account; reboot when prompted.
4. Verify in an elevated PowerShell:
   ```powershell
   dsregcmd /status
   ```
   Needs `AzureAdJoined : YES` and a populated `MdmUrl`. If `MdmUrl` is
   empty, enable Intune auto-enrollment: Entra admin → Mobility (MDM and
   MAM) → Microsoft Intune → **MDM user scope** = `All`, then unjoin +
   rejoin the device.

---

## Phase 2 — Add the device to `cmtraceopen-testdevices`

From your Mac (paste the hostname from step 1.2):

```bash
pwsh -Command '
Connect-MgGraph -Scopes GroupMember.ReadWrite.All,Device.Read.All -NoWelcome
$name = Read-Host "Windows hostname"
$dev  = Get-MgDevice -Filter "displayName eq ''$name''"
if (-not $dev) { throw "Device ''$name'' not found in Entra (wait ~1 min after enrollment)." }
New-MgGroupMember -GroupId 533f1ce3-1ac6-4c37-9d02-5938faf2cf03 -DirectoryObjectId $dev.Id
"Added $($dev.DisplayName) ($($dev.Id)) to cmtraceopen-testdevices."
'
```

Portal equivalent: Entra ID → Groups → `cmtraceopen-testdevices` →
**Members** → Add.

Force a sync: Intune admin → Devices → Windows → your device → **Sync**.

---

## Phase 3 — Wait for the Cloud PKI client cert to land

5–30 min after the sync fires. On the Windows box:

```powershell
Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.Issuer -match 'issuing\.gell\.internal\.cdw\.lab' } |
  Select-Object Subject, Thumbprint, NotAfter,
                @{N='EKU';E={($_.EnhancedKeyUsageList.FriendlyName) -join ','}},
                @{N='SAN';E={($_.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.17' } | ForEach-Object { $_.Format($false) }) -join ' | '}}
```

Should return one row with `Client Authentication` in the EKU column and a
SAN URI of the form `URL=device://<tenant-guid>/<aad-device-id>`.

If nothing lands after 30 min, Intune admin → Devices → your device →
**Device configuration** shows per-profile status and error messages.

---

## Phase 4 — Copy + install the MSI

Copy `CMTraceOpenAgent-0.1.0.msi` (or later version) to the Windows box,
e.g. `C:\Install\`. Install from an **elevated** PowerShell:

```powershell
msiexec /i C:\Install\CMTraceOpenAgent-0.1.0.msi /qn /l*v C:\Install\msi.log
```

`/qn` = silent, `/l*v <path>` = full verbose log (useful if anything goes
sideways). Verify the service is registered:

```powershell
Get-Service CMTraceOpenAgent | Format-List Name, Status, StartType
```

It'll be **Stopped** until you write config.toml — intended, the agent
refuses to start against the placeholder `https://api.corp.example.com`
default endpoint.

---

## Phase 5 — Configure the agent (point at your api-server)

```powershell
$cfg = @"
# cmtraceopen-agent configuration -- %ProgramData%\CMTraceOpen\Agent\config.toml

# Base URL of the api-server, reachable from this device.
# Plain HTTP is accepted; mTLS (https://) support lands when the api-server
# flips on CMTRACE_TLS_ENABLED.
api_endpoint = "http://<mac-ip>:8080"

# Override the device_id. Omit to fall back to the Windows hostname, which
# matches how DEV-WINARM64-01 and WIN-SMOKE appear today.
# device_id = "my-test-box-01"

[collection.schedule]
mode = "interval"
interval_hours = 1
jitter_minutes = 5
"@

# Agent expects its config at this path; MSI creates the directory.
$cfgPath = 'C:\ProgramData\CMTraceOpen\Agent\config.toml'
Set-Content -Path $cfgPath -Value $cfg -Encoding utf8

# Sanity-check the file exists and has no obvious typos:
Get-Content $cfgPath

# Start the service.
Start-Service CMTraceOpenAgent
Get-Service CMTraceOpenAgent
```

Replace `<mac-ip>` with the Mac's LAN IP you verified in Phase 0.

---

## Phase 6 — Verify the device reports in

On the Mac, in a browser hit http://localhost:5173/ (the viewer you already
have running), switch to the **Devices** tab, and wait ≤ 1 × interval_hours
+ jitter for the first bundle to post. Your new device should appear
alongside `DEV-WINARM64-01` and `WIN-SMOKE`.

Agent-side troubleshooting, on the Windows box:

```powershell
# Service state + recent events
Get-Service CMTraceOpenAgent
Get-WinEvent -LogName Application -MaxEvents 50 |
  Where-Object { $_.ProviderName -match 'CMTraceOpen|.NET Runtime' } |
  Select-Object TimeCreated, LevelDisplayName, Message | Format-List

# Agent logs (rotated, plain text)
Get-ChildItem C:\ProgramData\CMTraceOpen\Agent\logs\ | Sort-Object LastWriteTime -Descending | Select-Object -First 3
Get-Content (Get-ChildItem C:\ProgramData\CMTraceOpen\Agent\logs\ | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName -Tail 50

# Outbound connectivity to api-server
Test-NetConnection <mac-ip> -Port 8080
```

If the agent logs show `401 Unauthorized` on POST, the api-server is
enforcing Entra JWT auth — on HTTP ingest the agent falls back to
`X-Device-Id`, which requires the api-server to be in `CMTRACE_AUTH_MODE=disabled`
OR on a separate ingest path that skips operator auth. (The current
docker-compose has `CMTRACE_AUTH_MODE=enabled` gated by the viewer's JWT;
the ingest route is a different auth surface — check `crates/api-server/src/auth/device_identity.rs`.)

---

## Reset / redeploy

Uninstall (keeps config.toml + queue; see the WiX `KEEP_USER_DATA` property
in `docs/wave4/01-msi-design.md`):

```powershell
msiexec /x C:\Install\CMTraceOpenAgent-0.1.0.msi /qn
```

Full purge (deletes `%ProgramData%\CMTraceOpen\Agent\`):

```powershell
msiexec /x C:\Install\CMTraceOpenAgent-0.1.0.msi KEEP_USER_DATA=0 /qn
```
