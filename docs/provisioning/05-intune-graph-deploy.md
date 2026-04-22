# 05 — Intune Graph Deployment of the cmtraceopen Agent

Runbook for the **Wave 4** rollout of the cmtraceopen-agent MSI to a
fleet of Intune-managed Windows devices via Microsoft Graph automation.

This is the runbook that ties the previous four together: the
[Entra app registration](./02-entra-app-registration.md) supplies
operator OAuth, the [Cloud PKI cert profile](./03-intune-cloud-pki.md)
deploys the mTLS client cert, the
[Windows test VM](./04-windows-test-vm.md) is the target endpoint, and
this runbook is the script-driven push that lands the agent itself.

> Doc 05 in the provisioning series. Read it after 03 (cert profile) and
> 04 (test VM) — both are hard prerequisites.

---

## Section 1 — Goal

Stand up a one-shot deploy verb that ships the agent MSI into a target
Entra device group:

```powershell
pwsh ./Deploy-CmtraceAgent.ps1 `
    -DeviceGroupName 'cmtraceopen-testdevices' `
    -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
    -MsiProductCode '{12345678-1234-1234-1234-123456789012}'
```

**Success criteria.** Within 30 minutes of the deploy returning, every
device in the target group reports the agent as installed in the Intune
portal *and* surfaces in the api-server's `/v1/devices` endpoint:

```bash
curl -s http://192.168.2.50:8080/v1/devices | jq '.[] | select(.device_id != null)'
```

Each device row is the proof that the cert profile + MSI install + agent
service start + first bundle upload all completed end-to-end.

> **Note:** the agent service is `CMTraceOpenAgent`, runs as
> `LocalSystem` with `start= delayed-auto`, and writes its config to
> `%ProgramData%\CMTraceOpen\Agent\config.toml`. The MSI is responsible
> for creating the service, installing the binary into
> `C:\Program Files\CMTraceOpen\Agent\`, and laying down the default
> config. See `crates/agent/README.md` for the agent's own startup
> contract.

---

## Section 2 — Prereqs

Before running the scripts, confirm:

1. **PowerShell 7+** on the workstation that runs the deploy:

   ```powershell
   $PSVersionTable.PSVersion
   ```

2. **Microsoft.Graph SDK** installed:

   ```powershell
   Install-Module Microsoft.Graph -Scope CurrentUser
   ```

   Reference: <https://learn.microsoft.com/powershell/microsoftgraph/installation>.

3. **`IntuneWinAppUtil.exe`** on PATH, **or** allow `Pack-CmtraceAgent.ps1`
   to fetch it on first run (it caches under
   `tools/intune-deploy/.bin/`). Source:
   <https://github.com/microsoft/Microsoft-Win32-Content-Prep-Tool>.

4. **Cloud PKI cert profile assigned** to the target device group, per
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 4. The
   deploy script will warn if it can't see one — but won't fail. An
   agent without a cert can install but cannot mTLS-auth to the
   api-server.

5. **Agent MSI built.** The WiX MSI project is a **separate PR** (see
   "What's not in" in the Wave 4 tracking PR). Until that lands, use a
   placeholder MSI for `-DryRun` validation; the real deploy needs the
   signed MSI from the Wave 4 build pipeline.

6. **Target device group exists** in Entra. Recommended:
   `cmtraceopen-testdevices` (the same group used in runbook 03 — keeps
   cert + app assignments paired).

7. **Auth method chosen:**

   - **Interactive** (default) — opens a browser; the signed-in account
     needs `DeviceManagementApps.ReadWrite.All`,
     `DeviceManagementConfiguration.ReadWrite.All`, `GroupMember.Read.All`,
     and `Group.Read.All` consent.
   - **App-only** (CI / unattended) — pass `-TenantId`, `-ClientId`,
     `-ClientSecret`. The Entra app registration must have the same
     four permissions granted as **Application** scopes (not Delegated)
     with admin consent. See
     [`02-entra-app-registration.md`](./02-entra-app-registration.md)
     for the registration shape — Wave 4 needs a *separate* registration
     from the operator OAuth one (different audience, different
     consent boundary).

> **Note:** the deploy script never reads or writes the agent MSI's
> source code, signing keys, or config payload. It only orchestrates
> what Intune does with a pre-built `.intunewin`. If the install
> misbehaves, the fix is in the WiX project, not in this runbook.

---

## Section 3 — Deployment shape

```
                                  Wave 4 deploy orchestration

   tenant admin                                                                operator
        │                                                                          │
        │ (one-time, manual)                                                       │ pwsh ./Deploy-CmtraceAgent.ps1
        ▼                                                                          ▼
  ┌──────────────────────────┐                                       ┌──────────────────────────┐
  │ Cloud PKI cert profile   │                                       │ Pack-CmtraceAgent.ps1    │
  │ assigned to device group │                                       │  → IntuneWinAppUtil.exe  │
  │ (runbook 03)             │                                       │  → CMTraceOpenAgent      │
  └──────────────────────────┘                                       │      .intunewin          │
                │                                                    └────────────┬─────────────┘
                │                                                                 │
                │                                                                 ▼
                │                                                ┌─────────────────────────────────┐
                │                                                │  Microsoft Graph                │
                │                                                │  POST mobileApps (win32LobApp)  │
                │                                                │  POST contentVersions           │
                │                                                │  POST contentVersions/files     │
                │                                                │  PUT  Azure blob (chunked, 6MB) │
                │                                                │  POST files/commit              │
                │                                                │  POST mobileApps/assignments    │
                │                                                └────────────┬────────────────────┘
                │                                                             │
                ▼                                                             ▼
  ┌────────────────────────────────────────────────────────────────────────────────────────┐
  │                            Intune service (tenant-side)                                │
  └─────────────────────────────────────────────┬──────────────────────────────────────────┘
                                                │ device check-in (every 8h, or forced)
                                                ▼
  ┌────────────────────────────────────────────────────────────────────────────────────────┐
  │                          Windows endpoint (Entra-joined, Intune-enrolled)              │
  │                                                                                        │
  │   1. Cloud PKI client cert lands in LocalMachine\My                                    │
  │   2. MSI downloads, msiexec /i ... /qn runs as SYSTEM                                  │
  │   3. CMTraceOpenAgent service registered (delayed-auto, LocalSystem) and started       │
  │   4. Agent reads %ProgramData%\CMTraceOpen\Agent\config.toml                           │
  │   5. Agent finds cert in LocalMachine\My by issuer-pattern match                       │
  │   6. Agent ships first bundle  →  api-server (mTLS, port 8080)                         │
  │   7. /v1/devices row appears                                                           │
  └────────────────────────────────────────────────────────────────────────────────────────┘
```

The ordering between steps 1 and 2 is **not guaranteed**. If the MSI
installs first, the agent will start, fail to find a cert, log the
error, and retry on its next poll. Once Intune delivers the cert
profile, the next poll succeeds. This is fine — the agent is designed
to be cert-tolerant in start-up so we can deploy in either order.

---

## Section 4 — Step-by-step

### 4.1 Pack the MSI into a `.intunewin`

From the workstation that has the built (signed) MSI:

```powershell
cd <repo-root>/tools/intune-deploy

pwsh ./Pack-CmtraceAgent.ps1 `
    -SourceFolder 'C:\build\msi-staging' `
    -OutputFolder 'C:\build\out'
```

Expected output (last line):

```
C:\build\out\CMTraceOpenAgent.intunewin
```

Capture that path — it is the input to the next step.

> **Note:** `IntuneWinAppUtil.exe` is downloaded on first run if it
> isn't on PATH. Until a SHA256 is pinned in `Pack-CmtraceAgent.ps1`,
> the script will print the downloaded hash as a warning and ask you
> to pin it. Do this on a trusted workstation.

### 4.2 Dry-run the deploy

```powershell
pwsh ./Deploy-CmtraceAgent.ps1 `
    -DeviceGroupName 'cmtraceopen-testdevices' `
    -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
    -MsiProductCode '{12345678-1234-1234-1234-123456789012}' `
    -DryRun
```

Validates:

- Graph credentials work and the right scopes are consented.
- The device group exists.
- The Cloud PKI cert profile is assigned to it (warns if not).
- The `.intunewin` payload exists.
- The MSI ProductCode is a well-formed GUID.

No app is created and no assignment is made.

### 4.3 Deploy for real

Drop `-DryRun` and rerun:

```powershell
pwsh ./Deploy-CmtraceAgent.ps1 `
    -DeviceGroupName 'cmtraceopen-testdevices' `
    -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
    -MsiProductCode '{12345678-1234-1234-1234-123456789012}'
```

The summary at the end prints the new app id, the content version id,
the assignment count, and a link straight to the Intune portal page for
the new app.

### 4.4 Monitor in the Intune portal

Open the URL printed in the summary, then:

- **Device install status** tab — devices report `Pending`, `Installing`,
  `Installed`, or `Failed` as they check in.
- **User install status** is empty (this app is device-targeted).
- The `Required` column on the **Properties** tab should show the
  target device group.

Reference: <https://learn.microsoft.com/mem/intune/apps/apps-monitor>.

---

## Section 5 — Verification

### 5.1 Wait for Intune sync

Devices that are checked-in regularly pick up new app assignments within
**5–30 minutes**. To force a sync sooner:

- **Per-device, from the portal** — Devices → pick the device → Sync.
- **From the device** — Settings → Accounts → Access work or school →
  click the work account → Info → Sync.

### 5.2 Look for failures

In priority order:

1. **Intune portal — Device install status** for the app. The error
   string here usually points straight at the cause (missing
   prerequisite, MSI exit code, detection rule mismatch).
2. **Intune device check-in logs** on the endpoint:
   `Event Viewer → Applications and Services Logs → Microsoft → Windows →
   DeviceManagement-Enterprise-Diagnostics-Provider → Admin`.
3. **Application Event Log** on the endpoint — `Source: MsiInstaller`
   gives the raw msiexec exit code if the install ran but failed.
4. **Agent's own logs** at `%ProgramData%\CMTraceOpen\Agent\logs\` once
   the install succeeds. If you see install success in Intune but no
   logs here, the service never started — check
   `Get-Service CMTraceOpenAgent` for the status.

### 5.3 Confirm the agent reached api-server

From any LAN host that can reach BigMac26:

```bash
gh api --hostname github.com /repos/adamgell/cmtraceopen-web/contents/README.md  # sanity check gh works

curl -s http://192.168.2.50:8080/v1/devices | jq '.[] | select(.device_id != null)'
```

A populated row for each deployed device — with `last_seen` within the
last hour — is the end-to-end success signal.

---

## Section 6 — Teardown

Removing the assignment is a Graph call; it does **not** uninstall the
agent on devices automatically.

> **Note:** Intune **only** uninstalls a Win32 app from a device when
> the assignment intent flips from `required` to `uninstall` (or the
> device leaves the targeted group). Deleting the app outright leaves
> already-installed copies in place. Plan teardown accordingly.

### 6.1 Soft teardown — remove just the assignment

```powershell
$AppId  = '<app-id-from-deploy-summary>'
Connect-MgGraph -Scopes 'DeviceManagementApps.ReadWrite.All'
$assignments = Invoke-MgGraphRequest -Method GET `
    -Uri "https://graph.microsoft.com/beta/deviceAppManagement/mobileApps/$AppId/assignments"
foreach ($a in $assignments.value) {
    Invoke-MgGraphRequest -Method DELETE `
        -Uri "https://graph.microsoft.com/beta/deviceAppManagement/mobileApps/$AppId/assignments/$($a.id)"
}
```

New devices that join the group will not get the agent. Existing
installs are untouched.

### 6.2 Hard teardown — flip to uninstall, wait, then delete the app

```powershell
$AppId   = '<app-id-from-deploy-summary>'
$GroupId = '<entra-group-id>'
Connect-MgGraph -Scopes 'DeviceManagementApps.ReadWrite.All'

# Replace the 'required' assignment with an 'uninstall' assignment.
# Devices pick up the uninstall on the next sync (5-30 min).
$body = @{
    '@odata.type' = '#microsoft.graph.mobileAppAssignment'
    intent        = 'uninstall'
    target        = @{
        '@odata.type' = '#microsoft.graph.groupAssignmentTarget'
        groupId       = $GroupId
    }
} | ConvertTo-Json -Depth 6
Invoke-MgGraphRequest -Method POST `
    -Uri "https://graph.microsoft.com/beta/deviceAppManagement/mobileApps/$AppId/assignments" `
    -Body $body

# Wait for devices to report uninstalled in the portal — typically <1h.
# Then delete the app entry itself:
Invoke-MgGraphRequest -Method DELETE `
    -Uri "https://graph.microsoft.com/beta/deviceAppManagement/mobileApps/$AppId"
```

To also revoke the Cloud PKI cert on each device, follow runbook 03's
revocation procedure — the cert profile is independent of the app.

---

## Section 7 — Caveats

### RBAC for the deploy app registration

The Entra app registration that runs `Deploy-CmtraceAgent.ps1` in
unattended mode needs **Application** permissions:

- `DeviceManagementApps.ReadWrite.All`
- `DeviceManagementConfiguration.ReadWrite.All`
- `GroupMember.Read.All`
- `Group.Read.All`

All four require **admin consent**. Anything less than this fails
mid-run with a 403 from the first Graph call that needs the missing
scope. Don't try to scope down individually — the upload flow touches
several Graph entities and Microsoft hasn't published a sub-scope split.

### Graph API throttling

The Win32 LOB upload makes a small constant number of Graph calls per
deploy (one app create, one content version, one file entry, one
commit, one assignment) but the **per-chunk PUT to Azure blob** can
fire thousands of requests for a large MSI. The Azure blob endpoint
has its own throttle limits independent of Graph; on >100 MiB payloads
expect occasional 503s and let the script retry. For a >1000-device
target group, throttling on the **assignment** call itself can hit;
batch the deploy across smaller sub-groups if so.

Reference: <https://learn.microsoft.com/graph/throttling>.

### Cloud PKI cert profile is *not* automated here

Runbook 03 documents the cert profile creation in the Intune portal,
not via Graph. There are two reasons this script doesn't try to
automate it:

1. The **root-CA upload** (BYOR mode) is a one-time tenant-setup task
   that needs a human with the root-CA private material in hand.
   Encoding that into a deploy script is a worse story than a runbook.
2. The Graph entities for Cloud PKI cert profiles live under
   `deviceConfigurations` with `@odata.type` variants that move
   between Intune service updates. Pinning to those types in
   automation would create a maintenance tax that's worse than the
   manual portal flow.

If a future Wave does want full automation, the right hook is a
sibling script (`Deploy-CertProfile.ps1`) that takes an issuing-CA
reference and creates the PKCS profile via
`POST /deviceManagement/deviceConfigurations`. Out of scope for Wave 4.

### `.intunewin` payload encryption

`IntuneWinAppUtil.exe` AES-encrypts the MSI inside the `.intunewin`
container. The decryption key + IV live in `Detection.xml` inside the
container (which is itself a renamed zip). The deploy script relays
this metadata to Intune in the `commit` call — Intune then re-encrypts
on its side before pushing to devices. If the `.intunewin` is corrupted
(e.g. truncated download, antivirus interference), the commit step
returns a `400` with `encryptionInfo` errors. Re-pack and retry.

Reference: <https://learn.microsoft.com/mem/intune/apps/apps-add-graph-api>.

---

## "Done" criteria

- [ ] `Pack-CmtraceAgent.ps1` runs to completion and produces a
      `.intunewin` whose path is printed.
- [ ] `Deploy-CmtraceAgent.ps1 -DryRun` exits cleanly, including the
      cert-profile-assignment check.
- [ ] `Deploy-CmtraceAgent.ps1` (no `-DryRun`) prints a summary with
      a non-placeholder `AppId`, an Intune portal URL, and an
      `AssignmentCount` of 1.
- [ ] Within 30 minutes, the target device shows the agent as
      `Installed` in the Intune portal.
- [ ] Within 30 minutes, the target device appears in
      `GET /v1/devices` on the api-server.
- [ ] Teardown procedure is a single Graph DELETE that returns 204.

---

## References

- Win32 LOB app upload via Graph:
  <https://learn.microsoft.com/mem/intune/apps/apps-add-graph-api>
- Win32 Content Prep Tool:
  <https://github.com/microsoft/Microsoft-Win32-Content-Prep-Tool>
- Microsoft.Graph PowerShell SDK install:
  <https://learn.microsoft.com/powershell/microsoftgraph/installation>
- App assignment intents (`available`, `required`, `uninstall`):
  <https://learn.microsoft.com/mem/intune/apps/apps-deploy>
- Graph throttling:
  <https://learn.microsoft.com/graph/throttling>
- Cloud PKI overview:
  <https://learn.microsoft.com/mem/intune/protect/microsoft-cloud-pki-overview>
- Sibling runbook — cert profile setup:
  [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md)
- Sibling runbook — Windows test VM:
  [`04-windows-test-vm.md`](./04-windows-test-vm.md)
