# intune-provision

Tier-3 runbook. Takes you from a Cloud PKI tenant with root + issuing CAs
already created to a **Windows box that holds both a Cloud PKI client-auth
cert (for the cmtraceopen agent) and a code-signing cert (for signtool
running on a GitHub Actions self-hosted runner)**.

Same box runs both roles in this flow. Adding more agent-only devices
later is a one-liner: join them to Intune and add them to the
`cmtraceopen-testdevices` group.

Companion deep-dive docs (already in this repo — read if you want the why,
not just the how):

- `docs/provisioning/03-intune-cloud-pki.md` — full Cloud PKI runbook
- `docs/provisioning/04-windows-test-vm.md` — agent-only VM provisioning
- `docs/wave4/07-build-vm-runbook.md` — code-signing build VM runbook
- `docs/wave4/02-code-signing.md` — signing strategy

---

## Phase 0 — Prereqs

- Cloud PKI root + issuing CAs active in Intune > Tenant admin > Cloud PKI.
  Note both display names; you'll pass them to the script.
- PowerShell 7+ (`pwsh`) on the **Mac where you run the script** (not the
  target Windows box — provisioning is remote via Graph).
- An Entra account with **Intune Administrator** role.
- A target Windows 11 / Server 2022 machine (physical or VM) with:
  - TPM 2.0 + Secure Boot enabled (vTPM is fine on a hypervisor).
  - Internet access to `*.microsoft.com`, `*.manage.microsoft.com`,
    `*.pki.azure.net`.

---

## Phase 1 — Export the root CA certificate

1. Open Intune admin center > Tenant administration > Cloud PKI.
2. Click the **root** CA (`Gell - PKI Root`).
3. Click **Download certificate** (downloads a `.cer`).
4. Save it somewhere local. Any path works; the script only needs to read it.

Example: `~/Downloads/gell-pki-root.cer`.

---

## Phase 2 — Capture the issuing CA's SCEP URL

1. Intune admin center > Tenant administration > Cloud PKI > click the
   **issuing** CA (`Gell - PKI Issuing`).
2. Scroll to **SCEP URI**. Copy the full URL — it looks like:

   ```
   https://{{CloudPKIFQDN}}/TrafficGateway/PassThroughRoutingService/CloudPki/CloudPkiService/Scep/<guid>/<guid>
   ```

3. `{{CloudPKIFQDN}}` stays as-is — Intune resolves it server-side when
   the profile is delivered to the device.

---

## Phase 3 — Run the provisioner

From the repo root on your Mac:

```bash
pwsh ./tools/intune-provision/Provision-CmtraceIntune.ps1 `
    -IssuingCaDisplayName 'Gell - PKI Issuing' `
    -RootCaDisplayName    'Gell - PKI Root' `
    -RootCaCertPath       ~/Downloads/gell-pki-root.cer `
    -ScepUrl              'https://{{CloudPKIFQDN}}/TrafficGateway/PassThroughRoutingService/CloudPki/CloudPkiService/Scep/<guid>/<guid>'
```

Browser opens for Entra sign-in; grant Graph the scopes it asks for. On
success you'll get a summary like:

```
Tenant                      <guid>
Trusted-root config id      <guid>
Client-auth profile id      <guid>
Code-signing profile id     <guid>
cmtraceopen-testdevices id       <guid>
cmtraceopen-build-machines id    <guid>
```

The script is **idempotent** — rerun it safely.

What was just created in your tenant:

| Object | Purpose |
| --- | --- |
| `cmtraceopen-testdevices` group | Members receive the client-auth cert. |
| `cmtraceopen-build-machines` group | Members receive the code-signing cert. |
| `cmtraceopen-pki-root-trust` device config | Installs the Cloud PKI **root** into `LocalMachine\Root` on any member device so the issuing chain validates. Assigned to **both** groups. |
| `cmtraceopen-client-cert` SCEP profile | Issues a client-auth leaf cert with `CN={{AADDeviceId}}` and SAN URI `device://{{TenantId}}/{{AADDeviceId}}`. Assigned to testdevices. |
| `cmtraceopen-codesign-cert` SCEP profile | Issues a code-signing leaf with `CN={{DeviceName}}-codesign`. Assigned to build-machines. |

---

## Phase 4 — Prepare the Windows box

On the target machine:

1. Install Windows 11 Pro / Enterprise or Windows Server 2022. Keep the
   default computer name — no rename needed. Entra tracks the device by
   its AAD device id, which is independent of the Windows hostname.
2. Settings > Accounts > **Access work or school** > **Connect** >
   **Join this device to Microsoft Entra ID** (NOT the "sign in" button).
   Sign in with a tenant account; reboot when prompted.
3. Verify in an elevated PowerShell:
   ```powershell
   dsregcmd /status
   ```
   Needs `AzureAdJoined : YES` and a populated `MdmUrl`. If `MdmUrl` is
   empty, Intune auto-enrollment is off at the tenant level — fix in
   Entra admin > Mobility (MDM and MAM) > Microsoft Intune > **MDM user
   scope** = `All` or `Some`, then unjoin + rejoin.

### Add the device to both Entra groups

From your Mac (paste the IDs the provisioner printed):

```powershell
pwsh -Command '
Connect-MgGraph -Scopes GroupMember.ReadWrite.All,Device.Read.All -NoWelcome
# Use the Windows computer name as-is (no rename needed).
$dev = Get-MgDevice -Filter "displayName eq ''<windows-hostname>''"
New-MgGroupMember -GroupId <testdevices-id>     -DirectoryObjectId $dev.Id
New-MgGroupMember -GroupId <build-machines-id>  -DirectoryObjectId $dev.Id
'
```

Portal equivalent: Entra ID > Groups > each group > **Members** > Add.

### Kick a sync, wait for certs

Intune admin center > Devices > <your device> > **Sync**. Wait 5–30 min,
then on the Windows box:

```powershell
Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.2' -or
                 $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3' } |
  Select-Object Subject, Thumbprint, NotAfter,
                @{N='EKU';E={($_.EnhancedKeyUsageList.FriendlyName) -join ','}}
```

You should see one row with both `Client Authentication` and
`Code Signing` in the EKU column (since both EKUs live on the combined
profile). If nothing shows after 30 min, Intune admin center >
Devices > the device > **Device configuration** shows per-profile status.

---

## Phase 5 — Install the GitHub Actions self-hosted runner

One script does everything. On the Windows box:

1. Go to **https://github.com/adamgell/cmtraceopen-web/settings/actions/runners/new**
   (Windows / x64) and copy the one-time token from the `--token` line.
   Token expires in ~1 hour.
2. Copy `tools/intune-provision/Install-CmtraceRunner.ps1` to the Windows
   box (or clone this repo there).
3. Open an **elevated** PowerShell and run:
   ```powershell
   .\Install-CmtraceRunner.ps1 -Token '<paste-token>'
   ```

The script:

- verifies Entra join + Intune enrollment
- verifies the code-signing cert is present (warns, doesn't fail, if not)
- downloads the latest `actions/runner` release
- configures against `adamgell/cmtraceopen-web` with labels
  `self-hosted,windows,cmtrace-build` and the box's current hostname as
  the runner name
- installs the runner as a Windows service and starts it
- grants `NT AUTHORITY\NETWORK SERVICE` read access to the code-signing
  cert's private key (so signtool in CI can use it)

Confirm at the GitHub runners page — your runner should show **Idle**.

---

## Phase 6 — Run the cmtrace agent on the same box

Once the client-auth cert is present, the agent can use it for mTLS to
the api-server. Two dev paths:

- **Agent against the existing `CMTRACE_AUTH_MODE=enabled` HTTP api-server
  in docker-compose** — set the agent's `CMTRACE_AGENT_DEVICE_ID` env and
  let it use the legacy `X-Device-Id` header path. No mTLS. Good for a
  first smoke test.
- **Agent against a TLS-enabled api-server** — flip `CMTRACE_TLS_ENABLED=true`,
  `CMTRACE_CLIENT_CA_BUNDLE=/path/to/issuing-ca.pem` (export the
  **issuing** CA cert from the portal; `cloud-pki-issuing.pem` in the
  docs), `CMTRACE_TLS_CERT_FILE` / `CMTRACE_TLS_KEY_FILE` for the server
  cert, and rebuild / restart. The agent on the Windows box will present
  its Cloud-PKI-issued client cert automatically.

Follow `docs/provisioning/04-windows-test-vm.md` §7–§9 for the agent
install / service registration / smoke test. The viewer's **Devices** tab
should show your new device by its AAD device id / computer name within
a minute of the first successful bundle upload.

---

## Gotchas

- **SAN URI is load-bearing.** The api-server's SAN parser expects
  `device://{tenant}/{aad-device-id}`. Intune does **not** support
  `{{TenantId}}` as a cert template variable — hardcode your tenant GUID
  as static text in the URI and only use `{{AAD_Device_ID}}` as the
  variable part, e.g.
  `device://00000000-0000-0000-0000-000000000000/{{AAD_Device_ID}}`.
- **SCEP profile creation returns 400?** Usually `keySize` passed as int
  instead of the string `"size2048"`, or a missing EKU OID. The script
  uses the string forms; rerun with `-Verbose` if you see the error.
- **Cert doesn't land.** Intune > Devices > the device > **Device
  configuration** lists the profile status. "Not applicable" usually
  means the device isn't in the right group yet. "Error" drills in to
  the specific SCEP failure — 99% of the time it's a connectivity /
  proxy issue blocking `*.pki.azure.net`.
- **Private key permissions.** `NT AUTHORITY\NetworkService` (the default
  runner service account) often can't read the code-signing cert until
  you grant it via MMC > Manage Private Keys.
- **Cert renewal is automatic at 80% of lifetime.** Client-auth cert
  renews at day 292; code-signing at 9.6 months. Old certs linger; new
  ones take over on next use.
