# cmtraceopen-agent — WiX MSI installer

WiX v4 source for `CMTraceOpenAgent.msi`. The full design spec lives at
[`docs/wave4/01-msi-design.md`](../../../../docs/wave4/01-msi-design.md).

## Directory layout

```
crates/agent/installer/wix/
├── README.md                  # this file
├── Variables.wxi              # version, UpgradeCode, install paths (preprocessor defines)
├── Product.wxs                # WiX v4 Package + MajorUpgrade + Feature graph
├── Files.wxs                  # binary, LICENSE.txt, README.txt
├── Service.wxs                # ServiceInstall + ServiceControl + recovery actions
├── Config.wxs                 # default config.toml, Queue/ + logs/ dirs + ACLs
├── CustomActions/
│   └── CertCheck.ps1          # Cloud PKI cert presence check (soft-warn, always exits 0)
└── build.ps1                  # wix.exe wrapper: -ReleaseBinary, -Version
```

## File-by-file purpose

| File / dir                  | What it owns                                                                                                                      |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `Variables.wxi`             | Preprocessor `<?define ?>` block — `Version`, `UpgradeCode`, install dirs, source paths.                                         |
| `Product.wxs`               | Top-level `<Package>` element. `MajorUpgrade`, `MediaTemplate`, top-level `Feature` graph, custom-action scheduling.             |
| `Files.wxs`                 | `cmtraceopen-agent.exe`, `LICENSE.txt`, `README.txt` — each in its own `<Component>`. Defines `AgentInstallDir` directory tree.  |
| `Service.wxs`               | `ServiceInstall` + `ServiceConfig` (delayed auto-start) + `util:ServiceConfig` (3× restart recovery) + `ServiceControl`.         |
| `Config.wxs`                | Default `config.toml` (`NeverOverwrite="yes"`, `Permanent="yes"`), `CreateFolder` for `Queue/` and `logs/`, ACL grants.          |
| `CustomActions/CertCheck.ps1` | Runs as a deferred MSI custom action. Warns if no Cloud PKI cert is present in `LocalMachine\My`. Always exits 0.              |
| `build.ps1`                 | Thin wrapper around `wix build` + optional `signtool sign`. Takes `-ReleaseBinary` and `-Version`.                               |

## How to build

### Prerequisites

```powershell
dotnet tool install --global wix
wix extension add WixToolset.Util.wixext
```

### Build unsigned MSI (local dev / pilot)

```powershell
# From repo root, after: cargo build -p agent --release --target x86_64-pc-windows-msvc
./crates/agent/installer/wix/build.ps1 `
  -ReleaseBinary target/x86_64-pc-windows-msvc/release/cmtraceopen-agent.exe `
  -Version 0.1.0
# Output: crates/agent/installer/wix/out/CMTraceOpenAgent-0.1.0.msi
```

### Build + sign MSI (CI / release)

```powershell
./crates/agent/installer/wix/build.ps1 `
  -ReleaseBinary target/x86_64-pc-windows-msvc/release/cmtraceopen-agent.exe `
  -Version 0.1.0 `
  -SignCertThumbprint $env:SIGN_CERT_THUMBPRINT
```

CI invokes the same `build.ps1` from `.github/workflows/agent-msi.yml`.

## Install / uninstall

```powershell
# Install silently, writing a full install log:
msiexec /i CMTraceOpenAgent-0.1.0.msi /qn /l*v install.log

# Upgrade in-place (new MSI, same UpgradeCode):
msiexec /i CMTraceOpenAgent-0.2.0.msi /qn /l*v upgrade.log

# Uninstall (keep %ProgramData% — config, queue, logs survive):
msiexec /x {ProductCode} /qn

# Uninstall + full purge of %ProgramData%:
msiexec /x {ProductCode} KEEP_USER_DATA=0 /qn
```

## Key design decisions

- **UpgradeCode is fixed forever:** `463FD20A-1029-448F-AE5B-F81C818861D0`.
  Changing it would orphan existing installs.
- **`config.toml` is never overwritten on upgrade** (`NeverOverwrite="yes"`,
  `Permanent="yes"`). Operator edits survive. Delete the file + run MSI
  repair to reset to defaults.
- **`Queue/` and `logs/` are not deleted on uninstall** by default. Pass
  `KEEP_USER_DATA=0` for a full purge.
- **CertCheck.ps1 always exits 0** — a missing Cloud PKI cert is a warning
  in the MSI log, not an install failure.

## What this directory does NOT own

- **The Intune Cloud PKI cert profile.** Deployed separately by Intune; see
  [`docs/provisioning/03-intune-cloud-pki.md`](../../../../docs/provisioning/03-intune-cloud-pki.md).
- **The Graph automation** that pushes MSI + cert profile to a device group.
  Separate Wave 4 deliverable.
- **Code-signing cert procurement.** See design doc §9.

## Cross-references

- Design spec: [`docs/wave4/01-msi-design.md`](../../../../docs/wave4/01-msi-design.md)
- Agent crate: [`crates/agent/`](../../)
- Cloud PKI runbook: [`docs/provisioning/03-intune-cloud-pki.md`](../../../../docs/provisioning/03-intune-cloud-pki.md)
- Signing design: [`docs/wave4/02a-sign-every-component.md`](../../../../docs/wave4/02a-sign-every-component.md)
