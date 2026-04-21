# 01 - Windows 11 Test VM Provisioning

> Doc 01 of 3 in the provisioning series. 02 covers Entra join, 03 covers Intune + Cloud PKI.

## Purpose

This runbook provisions a **dedicated Windows 11 test endpoint** for end-to-end validation of `cmtraceopen-agent`. It is **not** the developer VM. Its job is to impersonate a real fleet device:

- Receive the `cmtraceopen-agent` MSI as if deployed by Intune
- Be **Entra-joined** and **Intune-enrolled** so it can receive a Cloud PKI certificate
- Ship logs to the api-server and have them surface in the web console

Keep this VM **clean of dev tooling** (no Go toolchain, no Visual Studio, no MSI authoring tools). It should look like a standard user's laptop on day one.

---

## Option A - Parallels on macOS (recommended)

You already run one Parallels VM for development. Spin up a second, isolated VM alongside it.

### 1. Obtain Windows 11 media

- Easiest path: **Parallels Desktop -> File -> New -> "Get Windows 11 from Microsoft"**. Parallels downloads the official image and injects drivers.
- Alternative: download the Windows 11 ISO manually from `https://www.microsoft.com/software-download/windows11` and pick it in the New VM wizard.
- Choose **Windows 11 Pro** (Home does not support Intune MDM enrollment).

### 2. VM settings

| Setting | Value | Note |
|---|---|---|
| Name | `cmtraceopen-testvm-01` | Keeps it distinct from the dev VM in the Parallels Control Center |
| vCPU | 4 | Minimum for comfortable Intune enrollment + MSI install |
| RAM | 8 GB | Intune policy sync is noisy; 4 GB is tight |
| Disk | 64 GB (expanding) | Windows + updates + agent logs fit comfortably |
| Boot firmware | UEFI + TPM 2.0 | Parallels enables these by default on Windows 11; required for Entra join |

### 3. Network - Shared (NAT)

Under **Configure -> Hardware -> Network 1**, set **Source: Shared Network**. This NATs the VM behind the Mac host so it can reach the api-server on `BigMac26` at `192.168.2.50:8080` via the shared bridge.

After first boot, verify from an elevated PowerShell inside the VM:

```powershell
Test-NetConnection 192.168.2.50 -Port 8080
```

`TcpTestSucceeded : True` is the go/no-go.

### 4. Isolation - skip deep Parallels Tools integration

Parallels Tools installs automatically, which is fine for drivers and resolution. **Turn off** the integration features that leak host state into the VM - we want this VM to feel like a real, standalone fleet device:

- **Configure -> Options -> Sharing**: disable "Share Mac folders with Windows", "Share Windows folders with Mac", "Share cloud storage".
- **Configure -> Options -> Applications**: disable "Share Windows applications with Mac" and "Share Mac applications with Windows".
- **Configure -> Options -> Full Screen / Coherence**: leave Coherence off; run the VM in a plain window.
- Clipboard: **Configure -> Options -> Advanced** -> set **Shared clipboard** and **Preserve text formatting** to **off**. Copy/paste between host and VM should not work.

### 5. Disable disruptive Windows Update reboots

From an elevated console in the VM:

```powershell
sconfig
```

Select **5) Windows Update Settings** -> **M** (Manual). This stops the test VM rebooting during long enrollment/log runs. Re-enable before you ship any real findings.

### 6. Post-install checklist

- [ ] Edition is **Windows 11 Pro** (`winver` confirms)
- [ ] Local admin account created (username + password in the user's password manager - **never** committed)
- [ ] Remove bloat: **Settings -> Apps -> Installed apps** - uninstall Xbox, Solitaire, Clipchamp, Office trial, etc.
- [ ] **Enable RDP**: `Settings -> System -> Remote Desktop -> On`, then allow through Windows Firewall
- [ ] **Rename device** to `cmtraceopen-testvm-01` (`Settings -> System -> About -> Rename this PC`) and reboot
- [ ] Snapshot the VM in Parallels (**Actions -> Take Snapshot**, name: `clean-post-install`) so you can roll back between test runs

---

## Option B - Azure Windows 11 VM (remote alternative)

> **Caveat first.** The api-server lives on `BigMac26` at `192.168.2.50:8080`, a private LAN address. An Azure VM on the public internet **cannot reach it** without a site-to-site VPN, Tailscale, or similar overlay. Treat Option B as a placeholder for when a public test endpoint exists. **Use Option A today.**

### Prereqs

- Azure CLI 2.50+ signed in (`az login`)
- Subscription selected (`az account set --subscription "<YOUR_SUBSCRIPTION_ID>"`)

### Create

```bash
az group create \
  --name rg-cmtraceopen-test \
  --location eastus

az vm create \
  --resource-group rg-cmtraceopen-test \
  --name cmtraceopen-testvm-01 \
  --image microsoftwindowsdesktop:windows-11:win11-22h2-pro:latest \
  --size Standard_B2ms \
  --admin-username cmtadmin \
  --admin-password '<STRONG_PASSWORD_FROM_PASSWORD_MANAGER>' \
  --public-ip-sku Standard \
  --nsg-rule RDP
```

Notes:

- `Standard_B2ms` is a burstable 2 vCPU / 8 GB SKU. Fine for a test endpoint, not for sustained load.
- `--nsg-rule RDP` opens 3389 from anywhere. **Lock it down** to your home/office public IP:
  ```bash
  az network nsg rule update \
    --resource-group rg-cmtraceopen-test \
    --nsg-name cmtraceopen-testvm-01NSG \
    --name rdp \
    --source-address-prefixes <YOUR_PUBLIC_IP>/32
  ```
- Confirm the image SKU is still current: `az vm image list --publisher microsoftwindowsdesktop --offer windows-11 --sku win11-22h2-pro --all --output table`. Microsoft rotates these.

### Cost

- Running 24/7: roughly **$30 USD / month** at `Standard_B2ms` plus ~$3 for the managed disk and public IP.
- **Deallocate when idle**:
  ```bash
  az vm deallocate --resource-group rg-cmtraceopen-test --name cmtraceopen-testvm-01
  ```
  Deallocated VMs stop compute billing; disk + IP keep costing a few dollars/month.
- Tear down entirely: `az group delete --name rg-cmtraceopen-test --yes`.

---

## Prereqs before Intune enrollment (both options)

1. **Local admin user + password** - store in the user's password manager. **Do not** commit to this repo or paste into tickets.
2. **Entra tenant credentials** - username + password for an account that has permission to join devices to Entra.
3. **Connect the VM to Entra**:
   - `Settings -> Accounts -> Access work or school -> Connect`
   - Click the small **"Join this device to Microsoft Entra ID"** link (not the default "add account" button - that only adds a work account, it does not device-join).
   - Sign in with tenant credentials: `<user>@<YOUR_TENANT_DOMAIN>`
   - Reboot when prompted.
4. After reboot, sign in with the Entra account. If Intune auto-enrollment is configured on the tenant (see doc 03), the device will enroll on first sign-in; watch **Settings -> Accounts -> Access work or school -> Info** for policy sync status.

---

## Verification

Run from an elevated PowerShell **inside the test VM**:

```powershell
# 1. LAN reachability to api-server (Parallels path)
Test-NetConnection 192.168.2.50 -Port 8080

# 2. Health check
Invoke-WebRequest -UseBasicParsing http://192.168.2.50:8080/healthz | Select-Object StatusCode, Content
```

Expect `StatusCode : 200` and a small JSON body. This proves the VM can talk to the api-server when the agent is installed.

## "Done" criteria

- [ ] VM is named `cmtraceopen-testvm-01` and runs Windows 11 Pro
- [ ] VM is **Entra-joined** (`dsregcmd /status` shows `AzureAdJoined : YES`)
- [ ] Device appears in **Intune admin center -> Devices -> All devices**
- [ ] `Test-NetConnection 192.168.2.50 -Port 8080` returns `TcpTestSucceeded : True`
- [ ] Clean snapshot taken (`clean-post-install`) so test runs are repeatable

Proceed to `02-entra-join.md`.
