# 07 — Build VM Provisioning Runbook (cmtraceopen Code-Signing Pipeline)

Provisioning runbook for the dedicated **Windows build VM** that hosts the
GitHub Actions self-hosted runner used to sign cmtraceopen MSI / EXE / PS
artifacts. The VM is Intune-enrolled and receives an Intune Cloud PKI
certificate with the **Code Signing** EKU into `LocalMachine\My`. CI signs
artifacts via `signtool` against that cert; every Intune-managed device that
trusts the Cloud PKI root (i.e. the entire pilot fleet by design — see
[`02-code-signing.md`](./02-code-signing.md)) automatically trusts the
resulting signature.

> Doc 07 in the wave-4 series.
> Companion docs:
>
> - [`02-code-signing.md`](./02-code-signing.md) — overall signing strategy
>   (currently ATS-focused; the Cloud-PKI internal-signing rewrite is in
>   flight by a sibling agent — cross-reference both once that lands).
> - [`../provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md) —
>   Cloud PKI tenant + cert-profile shape this runbook reuses.
> - [`../provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md) —
>   Windows test-VM provisioning. Section 5–6 of that doc is the same
>   Entra-join / Intune-enroll pattern reused here in §5.

---

## 1. Goal

A dedicated Windows build VM that:

1. Is **Entra-joined + Intune-managed** in the cmtraceopen tenant.
2. Holds a **Cloud PKI code-signing certificate** in `LocalMachine\My`,
   chained to **Gell - PKI Root** via **Gell - PKI Issuing**, with EKU
   `Code Signing` (OID `1.3.6.1.5.5.7.3.3`).
3. Is registered as a **GitHub Actions self-hosted runner** with labels
   `self-hosted`, `windows`, `cmtrace-build`.
4. Is the only place CI signs production binaries — the cert never leaves
   the VM.

**Trust scope.** The signing cert chains to **Gell - PKI Root**, which
every Intune-managed device in this tenant already trusts (the root is
deployed via the same MDM channel that delivers the client-auth cert in
[`03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)).
Therefore signed artifacts are trusted across the entire pilot fleet
automatically — no per-device trust install needed. Confirmed against the
Wave-4 signing strategy in [`02-code-signing.md`](./02-code-signing.md).

> **Note:** this runbook is the **internal-PKI** signing path. The Wave-4
> signing strategy doc currently leans on **Azure Trusted Signing (ATS)**
> for *public-trust* signatures (anything an external user might
> double-click). The two are complementary: ATS for public, Cloud PKI for
> internal-fleet artifacts. The strategy doc is being rewritten to make
> that distinction explicit; until that PR merges, treat this runbook as
> authoritative for internal-fleet signing.

---

## 2. Prereqs

Before starting, confirm:

1. **Cloud PKI tenant provisioned** — issuing CA `Gell - PKI Issuing` is
   already up per [`03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)
   and the reference notes in `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`.
2. **Intune license** assigned to the user/device.
3. **Compute capacity** — one of:
   - **Azure** subscription with capacity for a small Windows VM
     (Standard B2s ~ $30/mo on-demand, ~ $10–15/mo as a spot instance).
   - **On-prem Hyper-V / VMware** on a dev box.
4. **GitHub repo admin** on `adamgell/cmtraceopen-web` (required to
   register a self-hosted runner).
5. **Graph API access** — either
   - interactive Intune-admin-center clickops (§6a), **or**
   - an Entra app registration with the application permissions in §6b
     (script / Terraform / CI path).
6. **PowerShell 7+** on whatever host runs the Graph wrapper script.
7. **Windows ADK** (or just the `signtool.exe` standalone) staged for
   §8 manual verification — typically at
   `C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe`.

---

## 3. VM specs

|                | Min                                  | Recommended                                                                             |
| -------------- | ------------------------------------ | --------------------------------------------------------------------------------------- |
| OS             | Windows Server 2022 Standard         | Windows Server 2022 Standard                                                            |
| vCPU           | 2                                    | 4                                                                                       |
| RAM            | 4 GB                                 | 8 GB                                                                                    |
| Disk           | 64 GB SSD                            | 128 GB SSD                                                                              |
| Firmware / TPM | UEFI + Secure Boot + vTPM 2.0        | Same                                                                                    |
| Network        | Outbound HTTPS to Intune + GitHub + crates.io + raw.githubusercontent.com + Cloud PKI CRL endpoints | Same; bridged or NAT both fine — no inbound ports needed |

### Why Server 2022 over Windows 11 Pro

The Server SKU is preferred for a build agent because:

- **Headless management story is clearer** — no consumer telemetry knobs
  to fight, no Microsoft Store, no built-in nag prompts.
- **No OOBE Microsoft-account requirement** — Server skips the `BYPASSNRO`
  workaround that doc 04 uses.
- **Intune treats Server 2022 the same as Windows 11** for cert profile
  delivery as long as it's enrolled in the MDM authority. Cloud PKI cert
  payload + EKU enforcement behave identically.
- **Update windows are easier to control** — Server defers feature updates
  by default; we don't want a feature update interrupting a release build.

If Windows 11 Pro is what's on hand, it works — but cap it with a deferred
Insider channel and disable Active Hours auto-restart.

---

## 4. Provision the VM

Pick whichever hypervisor your team already uses. All three end at the
same place: a powered-on Server 2022 VM with a local admin account.

### 4a — Azure

```bash
# One-liner. Spot instance for cost savings during pilot; eviction risk is
# acceptable because GitHub Actions self-hosted runner jobs auto-retry on
# the queue if the runner disappears mid-job.
az vm create \
  --resource-group cmtraceopen-build-rg \
  --name cmtraceopen-build-01 \
  --image MicrosoftWindowsServer:WindowsServer:2022-datacenter-azure-edition:latest \
  --size Standard_B2s \
  --priority Spot --max-price -1 --eviction-policy Deallocate \
  --admin-username cmtbuild \
  --admin-password '<set-and-rotate>' \
  --public-ip-sku Standard \
  --nsg-rule RDP
```

After the VM is up, RDP in once to set the hostname (`cmtraceopen-build-01`)
and snapshot.

### 4b — Hyper-V

`New-VM -Name cmtraceopen-build-01 -Generation 2 -MemoryStartupBytes 8GB
-NewVHDPath D:\VMs\build-01.vhdx -NewVHDSizeBytes 128GB -SwitchName
"Default Switch"` then `Set-VMSecurity -VMName cmtraceopen-build-01
-EnableTrustedPlatformModule $true`. Mount the Server 2022 ISO, complete a
vanilla install, set hostname.

### 4c — VMware Workstation

`File → New Virtual Machine → Custom → Hardware compatibility 19+ → ISO →
Windows Server 2022 → 4 vCPU / 8 GB RAM / 128 GB disk → Customize Hardware
→ add Trusted Platform Module → Finish`. Vanilla install, set hostname.

### 4d — Snapshot

After the OS is installed and renamed, **snapshot the VM** as
`clean-local-admin`. This is the rollback point if Entra join or cert
issuance goes sideways — rebuilding the OS is the expensive step.

---

## 5. Entra-join + Intune enrollment

Same shape as
[`04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md) §5–6.
Read those sections — they cover the exact UI clicks.

Quick-version:

1. **Settings → Accounts → Access work or school → Connect → "Join this
   device to Microsoft Entra ID"**.
2. Sign in with a tenant account that has device-join permission.
3. Reboot, sign in with the Entra account.

**Verification — both must be true:**

```powershell
dsregcmd /status
```

Required output:

```
AzureAdJoined : YES
DomainJoined  : NO
TenantId      : <your-tenant-guid>
DeviceId      : <aad-device-guid>     # capture — used in §6 to add the VM to the build group
...
MdmUrl        : https://enrollment.manage.microsoft.com/...   # MUST be populated
```

Empty `MdmUrl` means the device Entra-joined but Intune auto-enrollment
didn't fire — fix tenant-side MDM scope before continuing (see doc 04 §6
for the `Mobility (MDM and MAM) → MDM user scope = All` setting).

Capture the `DeviceId` GUID — you'll feed it into either the Intune Center
group dialog (§6a step 1) or the Graph `add member` call (§6b step 2).

---

## 6. Graph permissions — clickops vs scripted

Two paths to provision the cert profile + group + assignment. Pick **6a**
for a one-time pilot stand-up; pick **6b** if this will be re-run
(Terraform, CI, multiple build VMs across tenants, blue/green VM swaps).

### 6a — Clickops via Intune admin center

#### Step 1: Create the build-machines group

1. Browse to <https://portal.azure.com> → **Microsoft Entra ID → Groups
   → New group**.
2. Fill in:
   - **Group type:** Security
   - **Group name:** `cmtraceopen-build-machines`
   - **Membership type:** Assigned
3. **Members:** Add → search by `DeviceId` GUID captured from §5
   `dsregcmd /status` (or by display name `cmtraceopen-build-01`).
   Confirm the Object Type column shows **Device** (not User).
4. Create. Capture the group's **Object ID** for cross-referencing.

#### Step 2: Create the Cloud PKI cert profile

1. Browse to <https://intune.microsoft.com> → **Devices → Configuration
   → Create profile**.
2. **Platform:** Windows 10 and later. **Profile type:** Templates →
   **SCEP certificate**.
   (The Cloud PKI tenant exposes itself as a SCEP endpoint to Intune;
   this is the same plumbing as the client-auth profile in doc 03 — just
   a different EKU.)
3. **Configuration settings:**

   | Field                            | Value                                                              |
   | -------------------------------- | ------------------------------------------------------------------ |
   | Display name                     | `cmtraceopen-codesign-builder`                                     |
   | Certificate type                 | **Device**                                                         |
   | Subject name format              | `CN={{DeviceName}}-codesign`                                       |
   | Subject alternative name         | leave default (no SAN entries needed for code-signing)             |
   | Certificate validity period      | **1 year**                                                         |
   | Key storage provider             | Enroll to TPM KSP, otherwise fail                                  |
   | Key usage                        | **Digital signature**                                              |
   | Key size                         | 2048                                                               |
   | Hash algorithm                   | SHA-2                                                              |
   | Root certificate                 | select **Gell - PKI Root** from dropdown                           |
   | **Extended key usage**           | **Code Signing** (OID `1.3.6.1.5.5.7.3.3`)                         |
   | SCEP server URL                  | auto-populated by Cloud PKI (the URI from `reference_cloud_pki.md`) |

   > **Important:** EKU must be **Code Signing only**. Do NOT add
   > `Client Authentication` — that EKU is for the agent's mTLS cert,
   > not this one. Mixed-EKU certs make signtool happy but make
   > security review unhappy.

4. **Assignments:** include the `cmtraceopen-build-machines` group from
   step 1. **Exclude** the test-device group `cmtraceopen-testdevices`
   for safety.
5. Review + create.

#### Step 3: Force a sync

In Intune admin center → **Devices → All devices → cmtraceopen-build-01
→ Sync**. Cert lands in `LocalMachine\My` within 5–30 minutes.

### 6b — Scripted via Graph

This is the path the user explicitly wants documented in depth. Same end
state as 6a, but reproducible.

#### Required Graph scopes

**Delegated** (interactive admin running the wrapper script from a
workstation):

| Scope                                              | Why                                                              |
| -------------------------------------------------- | ---------------------------------------------------------------- |
| `Group.ReadWrite.All`                              | Create + manage the `cmtraceopen-build-machines` security group  |
| `Device.Read.All`                                  | Look up the build VM's Entra device object ID by display name    |
| `DeviceManagementConfiguration.ReadWrite.All`      | Create + assign the Cloud PKI cert profile                       |
| `DeviceManagementManagedDevices.Read.All`          | Verify enrollment + read managed-device IDs for the sync trigger |

**Application** (unattended CI / Terraform / scheduled re-runs): same
four scopes, but as **Application permissions** with admin consent. The
`Group.ReadWrite.All` application permission is broad — see "blast
radius" below.

> **Blast radius for the application permissions.** `Group.ReadWrite.All`
> can read/modify any group in the tenant; `Device.Read.All` reads the
> entire Entra device inventory. Restrict the SP via **Entra → Enterprise
> applications → cmtraceopen-build-provisioner → Properties → Assignment
> required = Yes**, then control which automation principals can
> impersonate it. For Group-scope APIs, also consider
> [Resource-Specific Consent / RBAC for Intune](https://learn.microsoft.com/mem/intune/fundamentals/role-based-access-control)
> instead of the broad Graph scopes — but RBAC adds setup complexity that
> often isn't worth it for a single build VM.

#### App registration steps (only if doing the unattended path)

1. <https://portal.azure.com> → **Entra ID → App registrations → New
   registration**.
   - **Name:** `cmtraceopen-build-provisioner`
   - **Supported account types:** Single tenant
   - **Redirect URI:** none
2. **API permissions → Add a permission → Microsoft Graph → Application
   permissions → add the four scopes above:**
   - `Group.ReadWrite.All`
   - `DeviceManagementConfiguration.ReadWrite.All`
   - `DeviceManagementManagedDevices.Read.All`
   - `Device.Read.All`
3. Click **Grant admin consent for {tenant}**. **Without this step,
   every Graph call returns 403** — most common gotcha when handing the
   script to a fresh tenant.
4. **Certificates & secrets:** two options, federated cred strongly
   recommended.

   - **Recommended — Federated credential (GitHub OIDC, no secret to
     rotate):**
     - **Issuer:** `https://token.actions.githubusercontent.com`
     - **Subject identifier:**
       `repo:adamgell/cmtraceopen-web:environment:build-vm-provisioning`
       (use a GitHub Environment scope so a malicious branch can't
       borrow the trust — the Environment requires manual approval
       gates if you wire them up).
     - **Audience:** `api://AzureADTokenExchange`
   - **Fallback — Client secret:** New client secret, 6-month expiry,
     rotate via the same flow. Store as a GitHub Actions secret named
     `CMTRACE_BUILD_PROVISIONER_SECRET`. Drift risk: secrets get stale,
     federated creds don't.

#### Graph endpoints + payloads

The wrapper script in `tools/intune-deploy/Provision-BuildVm.ps1`
(scaffold; live implementation lands when WiX MSI work merges — see
**§6c**) wraps these five calls. They're shown raw here so a reviewer
can re-implement in Terraform / `curl` / any other client without
re-reading PowerShell.

**1. Create the security group.**

```http
POST https://graph.microsoft.com/v1.0/groups
Content-Type: application/json

{
  "displayName": "cmtraceopen-build-machines",
  "description": "Build VMs that hold the Cloud PKI code-signing cert. Members get the cmtraceopen-codesign-builder cert profile assigned.",
  "mailEnabled": false,
  "mailNickname": "cmtraceopen-build-machines",
  "securityEnabled": true,
  "groupTypes": []
}
```

→ returns `{ "id": "<group-id>", ... }`. Save `group-id`.

**2. Look up the build VM's Entra device object ID, then add it as a member.**

```http
GET https://graph.microsoft.com/v1.0/devices?$filter=displayName eq 'cmtraceopen-build-01'
```

→ returns `{ "value": [ { "id": "<device-object-id>", "deviceId": "<aad-device-guid>", ... } ] }`.

> **Note:** the response gives back **two** IDs that look similar but
> aren't interchangeable: `id` is the **directory object ID** used in
> Graph references; `deviceId` is the **AAD device GUID** that
> `dsregcmd /status` prints. Group `members/$ref` wants the directory
> object ID (the `id` field).

```http
POST https://graph.microsoft.com/v1.0/groups/{group-id}/members/$ref
Content-Type: application/json

{
  "@odata.id": "https://graph.microsoft.com/v1.0/directoryObjects/{device-object-id}"
}
```

→ 204 on success.

**3. Create the SCEP / Cloud PKI cert profile.**

```http
POST https://graph.microsoft.com/beta/deviceManagement/deviceConfigurations
Content-Type: application/json

{
  "@odata.type": "#microsoft.graph.windows81SCEPCertificateProfile",
  "displayName": "cmtraceopen-codesign-builder",
  "description": "Cloud PKI Code Signing cert for the cmtraceopen build VM. Lands in LocalMachine\\My with EKU 1.3.6.1.5.5.7.3.3.",
  "certificateStore": "machine",
  "subjectNameFormat": "custom",
  "subjectNameFormatString": "CN={{DeviceName}}-codesign",
  "subjectAlternativeNameType": "none",
  "customSubjectAlternativeNames": [],
  "keyUsage": "digitalSignature",
  "keySize": "size2048",
  "hashAlgorithm": "sha2",
  "extendedKeyUsages": [
    {
      "name": "Code Signing",
      "objectIdentifier": "1.3.6.1.5.5.7.3.3"
    }
  ],
  "certificateValidityPeriodScale": "years",
  "certificateValidityPeriodValue": 1,
  "renewalThresholdPercentage": 20,
  "scepServerUrls": [
    "https://{{CloudPKIFQDN}}/TrafficGateway/PassThroughRoutingService/CloudPki/CloudPkiService/Scep/8ddc9fff-7d78-4f0b-811e-a6eeeda2fbc5/7ff044a8-9c28-4529-9d79-76bdb94df99d"
  ],
  "rootCertificate": {
    "@odata.id": "https://graph.microsoft.com/beta/deviceManagement/deviceConfigurations/{cloud-pki-root-trustedRootCertificate-id}"
  }
}
```

→ returns `{ "id": "<config-id>", ... }`. Save `config-id`.

> **Note — Cloud PKI Graph payload shape is beta-only and weird.** The
> profile type `windows81SCEPCertificateProfile` lives only on the
> `/beta` endpoint; `/v1.0` doesn't expose Cloud-PKI-shaped cert
> profiles at all. Several fields use **stringly-typed enums** (e.g.
> `keySize` is the literal string `"size2048"`, not the integer
> `2048`; `certificateValidityPeriodScale` takes `"days" | "months" |
> "years"` rather than ISO-8601). The `rootCertificate` field is a
> Graph reference to a **separately-uploaded trusted root CA
> deviceConfiguration object** — you can't inline the PEM. To find its
> ID, query
> `GET /beta/deviceManagement/deviceConfigurations?$filter=isof('microsoft.graph.windows81TrustedRootCertificate')`
> and pick the one whose `displayName` matches your Cloud PKI root
> name. The SCEP URL template still has the `{{CloudPKIFQDN}}`
> placeholder — Intune substitutes it server-side, do **not** try to
> resolve it client-side.

**4. Assign the profile to the build-machines group.**

```http
POST https://graph.microsoft.com/beta/deviceManagement/deviceConfigurations/{config-id}/assign
Content-Type: application/json

{
  "assignments": [
    {
      "target": {
        "@odata.type": "#microsoft.graph.groupAssignmentTarget",
        "groupId": "{group-id}"
      }
    }
  ]
}
```

→ 200 on success. Note `/assign` (singular `assignments` body) is the
correct verb — `/assignments` POST also exists but takes one assignment
at a time.

**5. Trigger an immediate device sync (vs waiting up to 8h for the natural cycle).**

```http
GET https://graph.microsoft.com/beta/deviceManagement/managedDevices?$filter=deviceName eq 'cmtraceopen-build-01'
```

→ returns the `managedDeviceId` (a different ID again — Intune's, not
Entra's).

```http
POST https://graph.microsoft.com/beta/deviceManagement/managedDevices/{managedDeviceId}/syncDevice
```

→ 204. Cert lands within 5–10 minutes typically; up to 30 if Cloud PKI
is under load.

#### 6c — PowerShell wrapper script

A `Provision-BuildVm.ps1` script is **scaffolded** in
`tools/intune-deploy/` (companion to the existing
`Deploy-CmtraceAgent.ps1`) that wraps the five calls above with
`Connect-MgGraph`, parameter validation, and a `-DryRun` switch. The
wrapper is **not** included in this PR — it's tracked as a follow-up so
this docs change doesn't drift the existing tooling. The contract the
script will honor:

| Param            | Meaning                                                    |
| ---------------- | ---------------------------------------------------------- |
| `-VmDisplayName` | Build VM Entra display name (e.g. `cmtraceopen-build-01`)  |
| `-GroupName`     | Security group to create / use (default `cmtraceopen-build-machines`) |
| `-RootCaName`    | Trusted root CA profile display name (default `Gell - PKI Root`) |
| `-TenantId`      | Required for app-only auth                                 |
| `-ClientId`      | Required for app-only / federated auth                     |
| `-DryRun`        | Skip mutating Graph calls; print payloads                  |

Auth path mirrors `Deploy-CmtraceAgent.ps1`: interactive
`Connect-MgGraph -Scopes ...` by default; pass `-ClientId` /
`-TenantId` for federated/app-only. Same scope list as §6b above.

---

## 7. Verify the cert landed

On the build VM, in PowerShell as admin:

```powershell
Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.EnhancedKeyUsageList.FriendlyName -contains 'Code Signing' } |
  Format-List Subject, Issuer, NotAfter, Thumbprint
```

Expected output:

```
Subject    : CN=cmtraceopen-build-01-codesign
Issuer     : CN=issuing.gell.internal.cdw.lab, O=Gell CDW Workspace Labs, ...
NotAfter   : <~365 days out>
Thumbprint : <40-char hex>
```

Cross-reference the Issuer string against
`~/.claude/projects/F--Repo/memory/reference_cloud_pki.md` — Subject CN
of `Gell - PKI Issuing` is `issuing.gell.internal.cdw.lab`.

If the cert is missing after 30 minutes:

- Confirm the device is in the `cmtraceopen-build-machines` group
  (Intune admin center → Groups → Members).
- Force another sync: Intune admin center → Devices → cmtraceopen-build-01
  → Sync.
- Check the per-device profile state: Devices → cmtraceopen-build-01 →
  Device configuration → look for `cmtraceopen-codesign-builder` and
  its status. `Error` row gives the reason (most common: SCEP URI
  unreachable from device, or template variable mis-substituted).

---

## 8. Test signing manually before wiring CI

Sanity-check that the cert signs and that the chain validates **before**
hooking up the GitHub Actions runner — diagnosing a broken signing pipeline
through Actions logs is much harder than diagnosing it on the VM directly.

```powershell
# 1. Pick the cert (auto-selects the newest with EKU = Code Signing)
$cert = Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.EnhancedKeyUsageList.FriendlyName -contains 'Code Signing' } |
  Sort-Object NotAfter -Descending |
  Select-Object -First 1

# 2. Build (or grab) any test EXE. notepad.exe works as a proof-of-life;
#    in CI you'd be signing the cmtraceopen-agent build output.
Copy-Item C:\Windows\System32\notepad.exe .\test.exe

# 3. Sign — /a auto-selects from LocalMachine\My; /tr is the timestamp
#    URL; /td matches the file digest alg.
$signtool = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"
& $signtool sign /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 /d "cmtraceopen test sign" .\test.exe

# 4. Verify — /pa = PE auth policy; /v = verbose; /all = check every signature.
& $signtool verify /pa /v /all .\test.exe
```

Expected verify output (abbreviated):

```
Signing Certificate Chain:
    Issued to: gell.internal.cdw.lab
    Issued by: gell.internal.cdw.lab
        Issued to: issuing.gell.internal.cdw.lab
        Issued by: gell.internal.cdw.lab
            Issued to: cmtraceopen-build-01-codesign
            Issued by: issuing.gell.internal.cdw.lab

The signature is timestamped: ...
Successfully verified: test.exe
```

Three certs in the chain: leaf → `Gell - PKI Issuing` → `Gell - PKI
Root`. If verify fails with `chain not trusted`, the validation host
doesn't have the Cloud PKI root in its Trusted Root store — that's
expected for a non-Intune-managed verifier and not an error for the
build-VM use case (the target devices are all Intune-managed and *do*
trust the root).

---

## 9. Register as a GitHub self-hosted runner

1. <https://github.com/adamgell/cmtraceopen-web> → **Settings → Actions
   → Runners → New self-hosted runner**. Pick **Windows x64**.
2. Run the displayed PowerShell `Download` and `Configure` scripts on
   the build VM, **as the same admin user that will run the service**.
   At the labels prompt, enter:

   ```
   self-hosted, windows, cmtrace-build
   ```

   (`self-hosted` and `windows` are auto-applied; `cmtrace-build` is
   the discriminator that future workflows pin via `runs-on`.)
3. Install as a Windows service so it survives reboots:

   ```powershell
   .\svc.cmd install
   .\svc.cmd start
   ```

   The service runs as `NT AUTHORITY\NETWORK SERVICE` by default.
   That account **can** read `LocalMachine\My` — no extra ACLs needed
   for signtool to find the cert.
4. Verify in GitHub → repo → Settings → Actions → Runners — the new
   runner shows with status **Idle** and labels `self-hosted, windows,
   cmtrace-build`.

---

## 10. CI label match

The forthcoming `agent-msi.yml` workflow targets this runner via:

```yaml
jobs:
  build-and-sign:
    runs-on: [self-hosted, windows, cmtrace-build]
```

GitHub matches on **all** labels, so the job lands on this VM and not on
GitHub-hosted runners (which couldn't access `LocalMachine\My` anyway).
Cross-reference [`02-code-signing.md`](./02-code-signing.md) §5 once the
internal-PKI signing rewrite lands — the workflow shape there will plug
into this runner.

---

## 11. Hardening

- **Single-purpose VM.** Never install other workloads (dev tooling,
  random scripts, ad-hoc agents). This box exists to hold a private key
  and run signtool.
- **Disable RDP after initial setup.** Manage via **Azure Bastion** or
  **just-in-time access** (Defender for Cloud → JIT). Removing the
  always-on RDP NSG rule eliminates the largest exposed surface.
- **Defender for Endpoint enabled.** Ships via Intune by default; verify
  in Intune admin center → Endpoint security → Antivirus.
- **Update windows pinned.** `Settings → Windows Update → Active hours`
  → set to `08:00–22:00 weekdays` so a patch reboot can't interrupt a
  release build mid-sign.
- **Snapshot weekly.** A compromise can then be reverted to a
  known-good baseline within minutes; the cert stays valid because
  Cloud PKI didn't revoke it.
- **No secret extraction allowed.** The PKCS profile in §6 specifies
  `Enroll to TPM KSP, otherwise fail`, so the private key is
  TPM-bound and cannot be exported. Confirm via:

  ```powershell
  $cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.EnhancedKeyUsageList.FriendlyName -contains 'Code Signing' } | Select-Object -First 1
  certutil -store My $cert.Thumbprint | Select-String 'Provider'
  # Should report Microsoft Platform Crypto Provider (= TPM KSP).
  ```

---

## 12. Cert rotation

Cloud PKI auto-renews the cert **~30 days before expiry** (the 20%
renewal threshold from the profile). The renewed cert has a different
**thumbprint** but the same **EKU** (`Code Signing`) and the same
issuer.

`signtool /a` auto-selects the **newest** cert in `LocalMachine\My` that
matches the requested EKU — so the day after renewal, the next CI run
picks up the new cert with **zero workflow changes**. The old cert stays
in the store until it expires; signed artifacts produced before rotation
remain valid because of the embedded timestamp.

If you ever want to pin to a specific thumbprint instead of `/a`,
replace the sign step with:

```powershell
& $signtool sign /sha1 <thumbprint> /fd sha256 /tr http://timestamp.digicert.com /td sha256 .\artifact.exe
```

…but then you also have to update the workflow on each rotation. Don't
do that unless you have a specific reason.

---

## 13. Failure modes + triage

| Symptom                                                        | Likely cause                                                                                  | Fix                                                                                                  |
| -------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Cert never showed up after enrollment                          | Device not in `cmtraceopen-build-machines`; or assignment hasn't propagated                   | Confirm group membership; force `Sync`; check Intune device record's Configuration tab               |
| signtool says "no certificates were found that met all the given criteria" | Cert is in `CurrentUser\My` instead of `LocalMachine\My`; or EKU missing                | Re-check the §7 query targets `LocalMachine`; re-check profile in §6 has Code Signing EKU            |
| signtool runs as a non-admin user and finds nothing            | LocalMachine store needs admin / `LocalSystem` / `NETWORK SERVICE`                            | Run signtool from an elevated shell; or run via the GitHub Actions runner service (`NETWORK SERVICE` reads LocalMachine fine) |
| Signature exists but chain validation fails on a target device | Target device doesn't trust the Cloud PKI root                                                | Verify the device is Intune-managed in the same tenant; the root deploys via the same MDM channel    |
| `signtool verify` fails with `A certificate chain processed, but terminated in a root certificate which is not trusted by the trust provider` | Local-machine verification, root not in local Trusted Root store | Expected for non-Intune-managed verifiers; not an error for the build VM use case                    |
| Self-hosted runner shows offline in GitHub                     | Service stopped (Defender quarantine, reboot loop)                                            | RDP into VM, `Get-Service "actions.runner.*"`, `Restart-Service` if stopped                          |
| Graph payload returns `400 Bad Request` on the cert profile    | `keySize` passed as int `2048` instead of string `"size2048"`; or `extendedKeyUsages` missing the OID | Re-check §6b step 3 — Cloud PKI Graph payload is stringly-typed                                |

---

## 14. Cost

| Hosting choice                                | Approx. monthly cost                       |
| --------------------------------------------- | ------------------------------------------ |
| Azure Standard B2s, **spot instance**         | ~$10–15/mo (eviction risk; jobs auto-retry) |
| Azure Standard B2s, **on-demand**             | ~$30/mo                                    |
| Azure Standard B2s, **reserved 1-year**       | ~$18/mo                                    |
| On-prem Hyper-V on a dev box                  | $0 (sunk cost)                             |
| VMware Workstation on a dev box               | $0 (sunk cost) — Workstation Pro free tier |

For the pilot, **Azure spot** is the right call — minimal cost, easy
snapshot/restore, and a runner outage during a spot eviction is
acceptable because no human-facing user depends on a release shipping
within minutes. For prod, switch to **reserved** to eliminate eviction
risk.

---

## 15. Open questions

- [ ] **VM hosting choice.** Azure spot for pilot is recommended.
      Confirm before provisioning.
- [ ] **One build VM vs HA pair.** Single VM = no signing during patch
      windows; HA pair doubles cost + complicates cert rotation
      (renewals on two VMs at slightly different times). Pilot single
      is fine.
- [ ] **Terraform module.** The user has Terraform — cross-reference the
      in-flight `infra/azure/` module and decide whether to add a
      `build-vm` submodule that wraps the §4a + §6b flow end-to-end.
- [ ] **Provision-BuildVm.ps1 implementation.** The §6c contract is
      sketched but the live implementation is deferred until the WiX
      MSI work merges (so we don't fork the existing
      `Deploy-CmtraceAgent.ps1` patterns mid-flight). Track in the
      plan file.
- [ ] **ATS coexistence.** Once
      [`02-code-signing.md`](./02-code-signing.md) is rewritten to make
      the internal-PKI vs ATS distinction explicit, this runbook should
      gain a §16 clarifying which artifacts get signed where.

---

## "Done" criteria

- [ ] VM runs Windows Server 2022, named `cmtraceopen-build-01`, on
      UEFI + Secure Boot + TPM 2.0.
- [ ] `dsregcmd /status` reports `AzureAdJoined : YES` and a populated
      `MdmUrl`.
- [ ] Device is a member of `cmtraceopen-build-machines` and visible in
      Intune admin center → Devices.
- [ ] Cloud PKI cert exists in `Cert:\LocalMachine\My` with
      `Subject = CN=cmtraceopen-build-01-codesign`, Issuer matching
      `Gell - PKI Issuing`, EKU `Code Signing` only, private key
      TPM-bound (non-exportable).
- [ ] `signtool sign /a` + `signtool verify /pa /v /all` round-trip
      passes against a test EXE and the chain ends at `Gell - PKI Root`.
- [ ] GitHub Actions runner shows **Idle** with labels `self-hosted,
      windows, cmtrace-build`.
- [ ] Snapshot `clean-local-admin` exists in the hypervisor for
      rollback; weekly snapshot cadence noted.

---

## References

- Intune Cloud PKI overview:
  <https://learn.microsoft.com/mem/intune/protect/microsoft-cloud-pki-overview>
- Graph `windows81SCEPCertificateProfile` (beta):
  <https://learn.microsoft.com/graph/api/resources/intune-deviceconfig-windows81scepcertificateprofile>
- Graph `deviceConfiguration` assign:
  <https://learn.microsoft.com/graph/api/intune-deviceconfig-deviceconfiguration-assign>
- Graph `managedDevice` syncDevice:
  <https://learn.microsoft.com/graph/api/intune-devices-manageddevice-syncdevice>
- GitHub OIDC + Azure federated credentials:
  <https://learn.microsoft.com/azure/developer/github/connect-from-azure-openid-connect>
- GitHub self-hosted runners:
  <https://docs.github.com/actions/hosting-your-own-runners/managing-self-hosted-runners/about-self-hosted-runners>
- signtool reference:
  <https://learn.microsoft.com/windows/win32/seccrypto/signtool>
- Project: code-signing strategy — [`02-code-signing.md`](./02-code-signing.md)
- Project: Cloud PKI runbook — [`../provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)
- Project: Windows test VM (template for §5) — [`../provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md)
