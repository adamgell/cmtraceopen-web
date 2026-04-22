# 04 — Windows Test VM for End-to-End Agent + mTLS Validation

Provisioning runbook for a **Windows 11 Pro test VM** that hosts a real
`cmtraceopen-agent` install, Entra-joins to the project tenant, picks up an
Intune Cloud PKI client cert, and ships a real bundle to the api-server on
BigMac26.

This is the runbook that closes the loop on Wave 3: **a real device, on a
real OS, hitting the api-server with a real bundle and (eventually) a real
Cloud PKI cert.**

> Doc 04 in the provisioning series.
> [`01-windows-test-vm.md`](./01-windows-test-vm.md) covers the original
> Parallels-on-macOS path. This doc is the Windows-host (Hyper-V / VirtualBox /
> VMware Workstation) flavor and is the canonical "how do I get a fresh test
> endpoint up?" runbook.
> [`02-entra-app-registration.md`](./02-entra-app-registration.md) and
> [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) are tenant-side
> prerequisites and must be done first if not already in place.

---

## Section 1 — Goal

Stand up a Windows 11 Pro VM that:

1. Is **Entra-joined** so it has a stable AAD device ID.
2. Is **Intune-enrolled** so it can receive an Intune Cloud PKI client
   certificate via the PKCS profile from
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md).
3. Hosts the `cmtraceopen-agent` Windows service (`CMTraceOpenAgent`,
   `LocalSystem`, automatic-delayed-start).
4. Ships at least one real bundle to the api-server on BigMac26 at
   `http://192.168.2.50:8080/` and surfaces as a row in
   `GET /v1/devices`.

**Success criteria.** From any host on the dev LAN:

```bash
curl -s http://192.168.2.50:8080/v1/devices | jq '.[] | select(.device_id == "cmtraceopen-testvm-04")'
```

returns a non-empty object. That row is the proof that a real Windows
endpoint hit the real api-server end-to-end.

> **Note:** today the agent talks **plaintext HTTP**; the reqwest TLS layer
> isn't wired yet (see `crates/agent/README.md` → "Not yet in scope"). The
> Cloud PKI cert is provisioned now so it's already in `LocalMachine\My`
> when the mTLS PR lands — no second VM rebuild needed.

---

## Section 2 — Prereqs

Before starting, confirm:

1. **Hypervisor** — one of:
   - **Hyper-V** (Windows 11 Pro / Enterprise / Education host; enable via
     `Settings → Apps → Optional features → More Windows features → Hyper-V`).
   - **VirtualBox** 7.0+ (free; works on any Windows host).
   - **VMware Workstation Pro** 17+ (free for personal use as of 2024).
2. **~50 GB free disk** on the host volume that backs the VM (60 GB minimum
   VM disk + Windows install temp + first Windows Update wave).
3. **Windows 11 Pro ISO** — official Microsoft download:
   <https://www.microsoft.com/software-download/windows11>. Pick
   "**Download Windows 11 Disk Image (ISO) for x64 devices**". Home edition
   does **not** support Intune MDM enrollment — must be Pro (or Enterprise /
   Education).
4. **Entra-tenant credentials** for an account that has permission to enroll
   devices in the target tenant. Same tenant as
   [`02-entra-app-registration.md`](./02-entra-app-registration.md) and
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md).
5. **Tenant prerequisites already done.** If you haven't yet, complete
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) first — at minimum
   the issuing-CA provision and the PKCS certificate profile assigned to
   the `cmtraceopen-testdevices` group, otherwise the cert never shows up
   on the VM.
6. **api-server reachable** on `http://192.168.2.50:8080/`. Verify from any
   LAN host:

   ```bash
   curl -sf http://192.168.2.50:8080/healthz
   ```

   Expect HTTP 200 + a small JSON body. If this fails, fix BigMac26 first;
   the rest of this runbook can't progress.

---

## Section 3 — VM specs

Two profiles. Pick **recommended** unless host constraints force you down
to **minimum**.

| Resource | Minimum | Recommended |
| --- | --- | --- |
| vCPU | 2 | 4 |
| RAM | 4 GB | 8 GB |
| Disk | 60 GB (dynamically expanding) | 80 GB (dynamically expanding) |
| Firmware | UEFI + Secure Boot | UEFI + Secure Boot |
| TPM | TPM 2.0 (vTPM) — **required** | TPM 2.0 (vTPM) — **required** |
| Network | Bridged or NAT, must reach `192.168.2.50:8080` | Bridged (preferred) |

> **Note:** Windows 11 install **fails on the OOBE screen without TPM 2.0
> and Secure Boot**. All three hypervisors above expose a vTPM:
> Hyper-V (`New-VM ... -Generation 2`, then `Set-VMSecurity ... -EnableTrustedPlatformModule $true`),
> VirtualBox (`Settings → System → Motherboard → Enable TPM 2.0`),
> VMware Workstation (`VM → Settings → Options → Access Control → Encrypt`,
> then add a `Trusted Platform Module` device).

### Network — pick the one that reaches BigMac26

- **Bridged** — VM gets its own IP on the LAN. Always works for reaching
  `192.168.2.50`. Preferred when the host is on a flat home/office LAN.
- **NAT** — VM is behind the host. Reachable to outbound `192.168.2.50`
  via routing on the host; *not* reachable inbound, which is fine — we
  never need inbound to the VM for this runbook.
- **Host-only / Internal** — **do not use**. The VM cannot see BigMac26
  and the smoke test will fail.

After the VM is up, verify reachability from inside the guest before
moving on:

```powershell
Test-NetConnection 192.168.2.50 -Port 8080
```

`TcpTestSucceeded : True` is the go/no-go for sections 4 onward.

### Naming

| Field | Value |
| --- | --- |
| VM name (in the hypervisor) | `cmtraceopen-testvm-04` |
| Windows computer name | `cmtraceopen-testvm-04` (`Settings → System → About → Rename this PC`) |

The "04" suffix tracks this runbook number, so multiple test endpoints
provisioned from different runbooks don't collide in the Intune All
devices view.

---

## Section 4 — Install Windows 11 Pro

Goal: minimal, vanilla install. **Local admin first; Entra join in
section 5.** Keeping these two phases separate makes it trivial to revert
to a clean snapshot when an Entra/Intune cycle goes sideways.

1. Boot the VM from the Windows 11 ISO.
2. **Language / keyboard** — defaults are fine.
3. **Install now** → **I don't have a product key** → **Windows 11 Pro**.
   The eventual license assignment comes via the Entra/Intune flow once
   the device is joined.
4. Accept the license, choose **Custom: Install Windows only**, install
   to the single (unallocated) disk Windows offers. Reboots happen
   automatically.
5. **OOBE** — region / keyboard defaults.
6. **Sign-in screen** — at `"Let's add your Microsoft account"`, **do
   not** sign in with the Entra account here. Two options:
   - **Easiest path:** at the network screen earlier in OOBE, hit
     `Shift+F10` → run `OOBE\BYPASSNRO` → the VM reboots into OOBE with
     the "I don't have internet" option enabled, which then unlocks the
     "**Continue with limited setup**" link. This drops you into a local
     account creation form.
   - **Alternative:** complete OOBE with a throwaway Microsoft account,
     then immediately disconnect it after first login (`Settings →
     Accounts → Your info → Sign in with a local account instead`).
   Either way, end up with a **local admin account** named e.g. `cmtadmin`.
7. **Privacy settings** — turn everything **off** (location, find my
   device, diagnostic data set to Required only, tailored experiences
   off, advertising ID off). Cortana is no longer in Win11 OOBE; if a
   "Let Cortana help you" prompt appears, decline.
8. First boot → **Settings → Windows Update** → install all pending
   updates and reboot until clean. Intune MDM enrollment behaves badly
   on a partially-patched OS.
9. Rename the device:

   ```powershell
   Rename-Computer -NewName cmtraceopen-testvm-04 -Restart
   ```

10. After reboot, confirm:

    ```powershell
    winver           # confirms "Windows 11 Pro"
    hostname         # returns cmtraceopen-testvm-04
    ```

11. **Snapshot the VM** in your hypervisor. Name it `clean-local-admin`.
    This is the safe rollback point if Entra join fails and you want to
    retry from a known-good base.

> **Note:** do **not** install Visual Studio, Rust toolchain, Git, or any
> other dev tooling on this VM. The whole point is for it to look like
> a typical fleet endpoint. The agent binary is built elsewhere and copied
> over (section 8).

---

## Section 5 — Entra-join the device

1. From the local admin account, open **Settings → Accounts → Access work
   or school**.
2. Click **Connect**.
3. **Important:** in the dialog, scroll down and click the small
   **"Join this device to Microsoft Entra ID"** link near the bottom.

   > **Note:** the big **Sign in** button at the top **only adds a work
   > account to the local user**. It does **not** device-join. If you
   > accidentally use it, click `Disconnect` and start over with the
   > device-join link.

4. Sign in with tenant credentials: `<user>@<your-tenant-domain>`.
5. Confirm the organization name on the "Make sure this is your
   organization" screen, click **Join**.
6. Reboot when prompted.
7. After reboot, sign in with the **Entra account** (not the local admin
   you used in section 4 — that account remains as a fallback).
8. Verify the join from an elevated PowerShell:

   ```powershell
   dsregcmd /status
   ```

   Confirm:

   ```
   AzureAdJoined : YES
   DomainJoined  : NO
   TenantName    : <your-tenant-display-name>
   TenantId      : <your-tenant-guid>
   DeviceId      : <aad-device-guid>
   ```

   Capture `DeviceId` — that GUID is what the Cloud PKI cert SAN URI
   will encode (`device://<TenantId>/<DeviceId>`, see
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 4).

> **Note:** the Entra-joining user must be allowed to join devices in the
> target tenant. By default this is "All users" in a fresh tenant; if
> the tenant admin has restricted device join (Entra → Devices → Device
> settings → "Users may join devices to Microsoft Entra ID"), the user
> needs to be on that allowlist. Get this fixed before retrying.

---

## Section 6 — Verify Intune enrollment

If the tenant has **MDM auto-enrollment** configured (it should — that's
a tenant-level Entra setting under `Mobility (MDM and MAM) → Microsoft
Intune → MDM user scope = All` or `Some`), Entra join from section 5
**automatically** triggers Intune enrollment within a minute or two.

1. Confirm the MDM URL was pushed:

   ```powershell
   dsregcmd /status
   ```

   Look in the **SSO State** section for:

   ```
   MdmUrl     : https://enrollment.manage.microsoft.com/...
   MdmTouUrl  : ...
   MdmComplianceUrl : ...
   ```

   Empty `MdmUrl` means MDM auto-enrollment is **not** firing — see the
   manual-trigger note below.

2. In **Settings → Accounts → Access work or school**, click your
   organization entry → **Info**. You should see:
   - **Connected to** `<tenant-display-name>`'s Azure AD
   - A **Device sync status** section with a recent timestamp
   - A **Sync** button — click it to force a policy sync now.

3. From the admin side: **Intune admin center → Devices → All devices**
   should list `cmtraceopen-testvm-04` within a few minutes of sync.
   Add the device to the `cmtraceopen-testdevices` security group from
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 2 if it
   isn't already (assigned-membership groups don't auto-add).

> **Note:** if `MdmUrl` is empty even after a manual sync, MDM
> auto-enrollment isn't enabled tenant-wide. Manual enrollment is
> available from **Settings → Accounts → Access work or school →
> Enroll only in device management**, but the cleaner fix is to turn
> on auto-enrollment in the tenant (Entra → Mobility (MDM and MAM)
> → Microsoft Intune → MDM user scope) and re-do sections 5–6 from a
> snapshot. Intune docs:
> <https://learn.microsoft.com/mem/intune/enrollment/quickstart-setup-auto-enrollment>.

---

## Section 7 — Wait for the Intune Cloud PKI cert

This step is gated on the PKCS certificate profile from
[`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 4 being assigned
to the `cmtraceopen-testdevices` group **and** this VM being a member of
that group (section 6 step 3).

1. Force a policy refresh on the VM:

   ```powershell
   dsregcmd /refreshprt
   # Then trigger a sync:
   #   Settings → Accounts → Access work or school → <org> → Info → Sync
   # or from CLI:
   Start-Process -FilePath "$env:windir\system32\DeviceEnroller.exe" `
     -ArgumentList "/o","$env:USERDOMAIN","/c","/b"
   ```

2. **Wait 15–30 minutes** for the PKCS profile to deliver on first sync.
   Cloud PKI first-issuance is slow; subsequent renewals are fast.

3. Look for the cert in the LocalMachine personal store. Issuer string
   matches whatever name was set in
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 3 — the
   placeholder used in that doc is `cmtraceopen-issuing-ca`, but the
   real CA chain in this tenant is the **Gell CDW Workspace Labs Root +
   Issuing CA** pair. Match whichever string actually appears.

   ```powershell
   Get-ChildItem Cert:\LocalMachine\My |
     Where-Object {
       $_.Issuer -like "*cmtraceopen-issuing-ca*" -or
       $_.Issuer -like "*Gell*"
     } |
     Select-Object Thumbprint, Subject, Issuer, NotAfter
   ```

   You're looking for a cert whose:
   - `Subject` is `CN=<aad-device-id>` — matches `dsregcmd /status` →
     `DeviceId` from section 5.
   - `Issuer` matches the issuing CA configured in section 3 of doc 03.
   - `NotAfter` is ~365 days out.

4. Confirm the SAN URI carries the device identity the api-server will
   parse:

   ```powershell
   $cert = Get-ChildItem Cert:\LocalMachine\My |
     Where-Object { $_.Subject -match 'CN=' -and $_.Issuer -like '*Gell*' } |
     Select-Object -First 1

   $cert.Extensions |
     Where-Object { $_.Oid.Value -eq '2.5.29.17' } |
     ForEach-Object { $_.Format($true) }
   ```

   Expected output line:

   ```
   URL=device://<tenant-id>/<aad-device-id>
   ```

   This is **load-bearing**: the api-server's SAN parser reads exactly
   this URI to derive `device_id` once mTLS termination is on. Mismatch
   here = device shows up as `unknown` in `/v1/devices`. See
   [`03-intune-cloud-pki.md`](./03-intune-cloud-pki.md) Step 5 for full
   verification commands including the non-exportable-private-key check.

> **Note:** if no cert appears after 30 minutes:
>  - Confirm the device is in `cmtraceopen-testdevices` (Intune admin
>    center → Groups → cmtraceopen-testdevices → Members).
>  - Check the per-device profile state: Intune admin center → Devices →
>    `cmtraceopen-testvm-04` → Device configuration → look for the PKCS
>    profile and its status (`Succeeded` / `Pending` / `Error`).
>  - `Error` typically means the issuing CA isn't reachable from the
>    device or the SAN/Subject template references a variable Intune
>    doesn't resolve. The **Per-setting status** view explains why.

---

## Section 8 — Install the agent

The agent is a Windows service named `CMTraceOpenAgent`, running as
`LocalSystem` with start mode `automatic-delayed-start`.

> **Note:** the `cmtraceopen-agent` MSI is **pending** — it's tracked in
> the project plan as the WiX packaging milestone and isn't merged yet.
> Until that lands, use the cargo-build-then-copy path below. When the
> MSI ships, the install collapses to `msiexec /i CMTraceOpenAgent.msi
> CMTRACE_API_ENDPOINT=http://192.168.2.50:8080 /qn`.

### 8a — Build the binary on a dev machine (not the VM)

On any dev box with the Rust toolchain:

```bash
cargo build -p agent --release
```

Output: `target/release/agent.exe` (Windows host) or
`target/x86_64-pc-windows-msvc/release/agent.exe` (cross-build from
Linux/macOS). Copy that single binary to the VM via your hypervisor's
shared-folder feature, RDP clipboard, or a one-shot SMB share. Drop it
at:

```
C:\Program Files\CMTraceOpen\Agent\agent.exe
```

> **Note:** keep the VM **clean** of Rust/Visual Studio. The whole point
> of having a separate test VM is to validate the deploy artifact behaves
> on a vanilla endpoint, not on a dev workstation.

### 8b — Lay down the config

Create `%ProgramData%\CMTraceOpen\Agent\config.toml`:

```toml
# Wave 2/3 dev config. mTLS-enabled endpoint comes once the agent
# learns to load the LocalMachine\My cert (Wave 3 client-side PR).

api_endpoint         = "http://192.168.2.50:8080"
request_timeout_secs = 60
evidence_schedule    = "0 3 * * *"
queue_max_bundles    = 50
log_level            = "info"
device_id            = "cmtraceopen-testvm-04"
```

Then create the queue dir the agent will write to (the agent creates it
on first run, but pre-creating it lets us ACL it first):

```powershell
New-Item -ItemType Directory -Force -Path 'C:\ProgramData\CMTraceOpen\Agent'
New-Item -ItemType Directory -Force -Path 'C:\ProgramData\cmtraceopen-agent\queue'
```

> **Note on cert selection.** The agent's TOML schema today (see
> `crates/agent/src/config.rs`) does **not** yet expose a cert-by-issuer
> selector — that lands with the Wave 3 client-side mTLS PR. When it
> ships it'll add a `client_cert.issuer_pattern` (or similar) field; this
> runbook will be updated to set it to the Gell issuing-CA pattern from
> section 7. For now the agent talks plaintext to `:8080` and the
> Cloud PKI cert just sits idle in `LocalMachine\My`, ready to be
> picked up later.

### 8c — Hand-test in `--oneshot` mode (sanity check before installing the service)

From an elevated PowerShell on the VM:

```powershell
& 'C:\Program Files\CMTraceOpen\Agent\agent.exe' --oneshot
```

Expected: the agent collects, zips, enqueues, uploads one bundle, and
exits with status 0. Logs go to stderr at `info` level. If this errors,
fix it before installing the service — diagnosing a service that won't
start is much harder than diagnosing a foreground crash.

### 8d — Install as a Windows service

```powershell
sc.exe create CMTraceOpenAgent `
  binPath= "\"C:\Program Files\CMTraceOpen\Agent\agent.exe\"" `
  start= delayed-auto `
  obj= LocalSystem `
  DisplayName= "CMTrace Open Agent"

sc.exe description CMTraceOpenAgent "Ships Windows management logs and evidence bundles to the CMTrace Open api-server."

sc.exe start CMTraceOpenAgent
sc.exe query CMTraceOpenAgent
```

`sc.exe query` should report `STATE : 4 RUNNING` within a few seconds.

> **Note:** the spaces after each `=` in `sc.exe create` are **required**
> — `sc.exe`'s argument parser is famously picky. Copy-paste the block
> verbatim.

> **Note:** the agent crate scaffold flags `windows_service::service_dispatcher`
> as "not yet wired" (see `crates/agent/README.md` → "Not yet in scope").
> Until the SCM integration PR lands, the binary launched by `sc.exe`
> runs as a foreground process under the service host and may log
> `service_main not implemented` warnings. The collection + upload loop
> still runs; the SCM start/stop integration is the only gap. Re-test
> service control once that PR merges.

---

## Section 9 — Smoke test

End-to-end proof: the VM ships a bundle, the api-server records it.

1. **From the VM**, force a fresh one-shot collection (or wait for the
   service's interval timer):

   ```powershell
   & 'C:\Program Files\CMTraceOpen\Agent\agent.exe' --oneshot
   ```

2. **From any host on the LAN** (the BigMac, your dev workstation, etc.),
   query the api-server:

   ```bash
   curl -s http://192.168.2.50:8080/v1/devices | jq .
   ```

   Expect `cmtraceopen-testvm-04` to appear in the array. Drill in:

   ```bash
   curl -s http://192.168.2.50:8080/v1/devices/cmtraceopen-testvm-04/sessions | jq .
   ```

   Expect at least one session whose `created_at` is within the last
   couple of minutes.

3. **Optional** — pull one log entry to confirm the bundle actually
   landed on disk on BigMac26:

   ```bash
   curl -s http://192.168.2.50:8080/v1/devices/cmtraceopen-testvm-04/sessions \
     | jq -r '.[0].session_id' \
     | xargs -I{} curl -s "http://192.168.2.50:8080/v1/sessions/{}/entries?limit=1" \
     | jq .
   ```

4. **Watch the agent log** while smoke-testing — the foreground oneshot
   prints upload progress. If running as a service, capture stderr by
   restarting the service with `sc.exe stop CMTraceOpenAgent` then
   running the binary in-foreground from the same elevated console.

> **Note:** if the device appears but sessions are empty, the upload
> chunked-finalize handshake errored mid-stream. Check the agent log
> for `4xx` / `5xx` from `/v1/ingest/bundles/...` and cross-reference
> the api-server log on BigMac26.

---

## Section 10 — Teardown

Two flavors. Use **soft teardown** between test runs; use **hard
teardown** when you want to repurpose the VM or free the disk.

### 10a — Soft teardown (revert between test runs)

1. Stop the agent:

   ```powershell
   sc.exe stop CMTraceOpenAgent
   ```
2. **Snapshot revert** — in your hypervisor, revert to the
   `clean-post-enrolled` snapshot (take one after section 7 succeeds —
   it captures Entra-joined + cert-issued state, which is the expensive
   part to rebuild).
3. The Intune device record stays in place, the Cloud PKI cert is still
   valid, and the next test cycle starts at section 8.

### 10b — Hard teardown (remove the VM from the tenant)

1. **From inside the VM**, leave Entra:

   ```powershell
   # Settings UI:
   #   Settings → Accounts → Access work or school → <org> → Disconnect
   # CLI alternative (requires admin):
   dsregcmd /leave
   ```

   Reboot, then sign in with the local admin account from section 4.

2. **From Intune admin center → Devices → All devices** → select
   `cmtraceopen-testvm-04` → **Delete**. This also removes the device
   record from Entra after a short delay.

3. **From Cloud PKI** (Intune admin center → Tenant administration →
   Cloud PKI → issuing CA → Certificates) → find the cert by serial /
   subject `CN=<aad-device-id>` → **Revoke**. Reason: `cessationOfOperation`
   or `superseded` is appropriate for a test-VM teardown.

4. **Power off and delete the VM** in the hypervisor. Reclaim the disk.

> **Note:** revoking the Cloud PKI cert ensures it can't be re-used if
> the VM disk is later restored from a backup. Skipping revoke leaves a
> "valid" cert floating around with no live device behind it, which
> defeats the point of having a CRL.

---

## "Done" criteria

- [ ] VM runs Windows 11 Pro, named `cmtraceopen-testvm-04`, on
      UEFI + Secure Boot + TPM 2.0.
- [ ] `Test-NetConnection 192.168.2.50 -Port 8080` returns
      `TcpTestSucceeded : True` from inside the VM.
- [ ] `dsregcmd /status` reports `AzureAdJoined : YES` and a populated
      `MdmUrl`.
- [ ] Device is a member of `cmtraceopen-testdevices` and visible in
      Intune admin center → Devices → All devices.
- [ ] A Cloud PKI cert exists in `Cert:\LocalMachine\My` with
      `Subject = CN=<aad-device-id>`, issuer matching the configured
      Cloud PKI issuing CA, EKU `Client Authentication` only, SAN URI
      `device://<tenant-id>/<aad-device-id>`, private key non-exportable.
- [ ] `agent.exe` is installed at
      `C:\Program Files\CMTraceOpen\Agent\agent.exe`, config exists at
      `%ProgramData%\CMTraceOpen\Agent\config.toml`.
- [ ] `sc.exe query CMTraceOpenAgent` reports `STATE : 4 RUNNING`.
- [ ] `GET http://192.168.2.50:8080/v1/devices` returns a row for
      `cmtraceopen-testvm-04`, and that device has at least one session
      with non-zero entries.
- [ ] A snapshot named `clean-post-enrolled` exists in the hypervisor so
      future test runs can soft-revert.

---

## References

- Windows 11 ISO download:
  <https://www.microsoft.com/software-download/windows11>
- Bypass Microsoft-account requirement at OOBE (`OOBE\BYPASSNRO`):
  <https://learn.microsoft.com/windows/deployment/usmt/usmt-overview>
- Entra device-join overview:
  <https://learn.microsoft.com/entra/identity/devices/concept-directory-join>
- `dsregcmd` reference:
  <https://learn.microsoft.com/entra/identity/devices/troubleshoot-device-dsregcmd>
- Intune MDM auto-enrollment:
  <https://learn.microsoft.com/mem/intune/enrollment/quickstart-setup-auto-enrollment>
- Hyper-V on Windows 11:
  <https://learn.microsoft.com/virtualization/hyper-v-on-windows/quick-start/enable-hyper-v>
- `sc.exe create` reference:
  <https://learn.microsoft.com/windows-server/administration/windows-commands/sc-create>
- Project: agent crate scaffold + config schema —
  `crates/agent/README.md`, `crates/agent/src/config.rs`.
- Time sync and network policy requirements for the agent —
  [`docs/wave4/21-agent-network-time.md`](../wave4/21-agent-network-time.md).
  VMs paused/restored often have skewed clocks; run `w32tm /resync /force`
  after restoring a snapshot and verify with `w32tm /query /status`.
