# 15 — Agent MSI CI Recipe

**Status:** Live. Workflows implemented at `.github/workflows/agent-msi.yml`
and `.github/workflows/sign-agent.yml`.

**Audience:** operators building, signing, and releasing the
`cmtraceopen-agent` Windows MSI installer.

---

## Overview

The CI pipeline for the agent MSI lives in two workflows:

| Workflow | Purpose |
|---|---|
| `.github/workflows/agent-msi.yml` | Full build → sign → publish pipeline. Triggered by `agent-v*` tags and `workflow_dispatch`. |
| `.github/workflows/sign-agent.yml` | Reusable signing workflow. Signs a previously uploaded artifact using signtool + the Cloud PKI cert. |

### Sign order

```
cargo build --release -p agent
    ↓ cmtraceopen-agent.exe
signtool sign (Cloud PKI cert, LocalMachine\My)
    ↓ signed cmtraceopen-agent.exe
Set-AuthenticodeSignature (CertCheck.ps1)
    ↓ signed CertCheck.ps1
crates/agent/installer/wix/build.ps1
    ↓ CMTraceOpenAgent-{version}.msi  (contains signed exe + signed script)
signtool sign (same cert)
    ↓ signed CMTraceOpenAgent-{version}.msi
signtool verify /pa /v /all  (all artifacts)
    ↓ verified
actions/upload-artifact  +  softprops/action-gh-release
```

---

## Prerequisites

The following must be in place before the workflow can succeed. Each has
a corresponding runbook section referenced below.

### 1. WiX MSI sources

`crates/agent/installer/wix/build.ps1` and the accompanying WiX `.wxs`
files must exist. These are implemented in a separate issue. Until they
land, the workflow will fail at the **Build MSI** step with a clear error.

### 2. Self-hosted runner

A Windows build VM with the runner registered under labels
`[self-hosted, windows, cmtrace-build]`.

Required software on the VM:

| Tool | Minimum version | Install notes |
|---|---|---|
| Windows SDK (signtool.exe) | 10.0.22000+ | "Windows SDK Signing Tools for Desktop Apps" component required |
| .NET SDK | 8.0+ | For `dotnet tool install --global wix` |
| Rust (stable) | 1.77.2+ | Via rustup; see `rust-toolchain.toml` |
| WiX v4 | 4.0+ | Installed automatically by the workflow via `dotnet tool install` |

See `docs/wave4/07-build-vm-runbook.md` for the full VM provisioning recipe.

### 3. Cloud PKI code-signing cert

The Intune Cloud PKI cert profile (issuer: `Gell - PKI Issuing CA`) must be
assigned to the build VM's device group and the cert must be present in
`LocalMachine\My`.

To verify on the build VM:

```powershell
Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.Issuer -match 'Gell.*PKI' -and $_.HasPrivateKey } |
  Format-List Subject, Issuer, Thumbprint, NotBefore, NotAfter
```

If no cert appears, trigger an Intune sync and wait up to 15 minutes:

```powershell
# Trigger Intune sync (run as SYSTEM or local admin):
Start-Process -Wait 'C:\Windows\System32\deviceenrollmentmgr.exe'
```

See `docs/wave4/02-code-signing.md §4` for the full cert-profile setup.

---

## Cutting a release

```bash
# 1. Ensure the version in crates/agent/Cargo.toml is correct.
#    Example: version = "0.1.0"

# 2. Tag the commit.
git tag agent-v0.1.0

# 3. Push the tag.
git push origin agent-v0.1.0
```

This triggers `agent-msi.yml` which:

1. Builds the agent binary in release mode.
2. Signs `cmtraceopen-agent.exe` and `CertCheck.ps1`.
3. Builds `CMTraceOpenAgent-0.1.0.msi` via `build.ps1`.
4. Signs the MSI.
5. Verifies all signed artifacts.
6. Uploads the MSI as a workflow artifact **and** attaches it to the
   GitHub Release created for the `agent-v0.1.0` tag.

---

## Manual / dry-run trigger

Use `workflow_dispatch` from the GitHub Actions UI or CLI:

```bash
# Build + sign (default)
gh workflow run agent-msi.yml

# Build + sign with explicit version (overrides Cargo.toml)
gh workflow run agent-msi.yml -f version=0.2.0-rc.1

# Build only, skip signing (useful when testing build changes)
gh workflow run agent-msi.yml -f sign=false
```

A `workflow_dispatch` run that succeeds with `sign=true` (the default)
satisfies the acceptance criterion "Manual workflow_dispatch run produces
a signed MSI in the workflow artifacts".

---

## Using the reusable `sign-agent.yml` workflow

`sign-agent.yml` is a standalone reusable workflow that can sign any
uploaded artifact with the Cloud PKI cert. To use it from another workflow:

```yaml
jobs:
  build:
    runs-on: [self-hosted, windows, cmtrace-build]
    steps:
      - name: Upload unsigned artifact
        uses: actions/upload-artifact@v4
        with:
          name: my-artifact.msi
          path: out/my-artifact.msi

  sign:
    needs: build
    uses: ./.github/workflows/sign-agent.yml
    with:
      artifact-name: my-artifact.msi
      description: "My Artifact Installer"
      file-glob: "*.msi"
```

The workflow re-uploads the signed artifact as `my-artifact.msi-signed`.

---

## Verifying a signed MSI locally

After downloading the MSI from the GitHub Release or workflow artifacts:

```powershell
# Full chain verification (should resolve to Gell PKI Issuing CA):
& signtool.exe verify /pa /v /all CMTraceOpenAgent-0.1.0.msi

# Expected output (abbreviated):
# Signing Certificate Chain:
#   Issued to: <Root CA>
#     Issued to: Gell - PKI Issuing CA
#       Issued to: <Subject CN>
# File is signed and timestamped.
# Successfully verified: CMTraceOpenAgent-0.1.0.msi
```

You can also right-click the MSI → Properties → Digital Signatures to
inspect the cert chain in the Windows UI.

---

## Troubleshooting

### "No valid Cloud PKI code-signing cert found in LocalMachine\My"

**Cause:** The Intune cert profile has not been assigned to this device's
group, the device has not synced since the assignment, or the cert has
expired.

**Fix:**
1. Open the Intune portal → Devices → Configuration → confirm the
   `codeSigning` cert profile is assigned to the build VM's group.
2. On the build VM, trigger a manual Intune sync and wait 10–15 minutes.
3. Re-run the workflow.

See `docs/wave4/02-code-signing.md §4`.

### "signtool.exe not found"

**Cause:** The Windows 10 SDK is not installed on the build VM, or the
"Windows SDK Signing Tools for Desktop Apps" component was not selected
during install.

**Fix:**
Install the Windows SDK from <https://developer.microsoft.com/windows/downloads/windows-sdk/>,
selecting the "Windows SDK Signing Tools for Desktop Apps" component.
After install, verify:

```powershell
Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin' -Recurse -Filter signtool.exe |
  Where-Object { $_.FullName -match '\\x64\\' }
```

### "build.ps1 not found"

**Cause:** The WiX MSI sources have not been merged yet (separate issue).

**Fix:** Wait for the WiX implementation PR to merge, then re-run.

### "signtool verify FAILED"

**Cause:** The cert chain is broken — this can happen if:
- The cert has expired.
- The Gell PKI Issuing CA root is not in the machine's trusted root store.
- The timestamp authority was unreachable at signing time (signature has
  no timestamp, so it becomes invalid after cert expiry).

**Fix:**
1. Check cert validity: `Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.Issuer -match 'Gell' } | Format-List NotAfter`
2. Confirm the Gell PKI root CA is in `Cert:\LocalMachine\Root`.
3. If the cert has expired, wait for Intune to renew it (Cloud PKI
   auto-renews near expiry) or manually request a new cert.

---

## Cross-references

- `docs/wave4/01-msi-design.md §10` — CI shape rationale
- `docs/wave4/02-code-signing.md` — full signing strategy
- `docs/wave4/02a-sign-every-component.md §5` — sign-order recipe
- `docs/wave4/07-build-vm-runbook.md` — build VM setup
- `.github/workflows/agent-msi.yml` — build + sign + publish pipeline
- `.github/workflows/sign-agent.yml` — reusable signing workflow
