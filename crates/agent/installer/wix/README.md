# cmtraceopen-agent — WiX MSI installer (scaffold)

This directory will hold the WiX v4 source for `CMTraceOpenAgent.msi`. **No
WiX source has been committed yet** — the design lives at
[`docs/wave4/01-msi-design.md`](../../../../docs/wave4/01-msi-design.md). The
implementation lands in a follow-up PR once the open questions in section 12
of that doc are answered.

This README documents the planned directory layout so the next contributor
knows where each file goes.

## Planned layout

```
crates/agent/installer/wix/
├── README.md                  # this file (the only file present today)
├── Variables.wxi              # version, UpgradeCode, package GUID, install paths
├── Product.wxs                # WiX v4 — Package + MajorUpgrade + Feature graph
├── Files.wxs                  # binary, LICENSE.txt, README.txt
├── Service.wxs                # ServiceInstall + ServiceControl + recovery actions
├── Config.wxs                 # default config.toml, Queue/ + logs/ dirs + ACLs
├── CustomActions/
│   ├── CertCheck.ps1          # OR
│   └── CertCheck/             #   C# DTF project — pick one (see design §6, §12)
└── build.ps1                  # wix.exe wrapper: -ReleaseBinary, -Version
```

## File-by-file purpose

| File / dir              | What it owns                                                                                                                  |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `Variables.wxi`         | Preprocessor `<?define ?>` block — `Version`, `UpgradeCode`, install dirs, source paths.                                      |
| `Product.wxs`           | Top-level `<Package>` element. Includes `MajorUpgrade`, `MediaTemplate`, top-level `Feature` graph, and references the other `.wxs`. |
| `Files.wxs`             | The `cmtraceopen-agent.exe` `<File>`, `LICENSE.txt`, `README.txt`. Each in its own `<Component>`.                             |
| `Service.wxs`           | `ServiceInstall` + `ServiceControl` + `util:ServiceConfig` recovery actions. KeyPath is the agent EXE.                        |
| `Config.wxs`            | Default `config.toml` payload (with `NeverOverwrite="yes"`), `CreateFolder` for `Queue/` and `logs/`, ACL grants.             |
| `CustomActions/`        | The Cloud PKI cert presence check. PS or DTF — see design doc §6.                                                              |
| `build.ps1`             | Thin wrapper around `wix build` + optional `signtool sign`. Takes `-ReleaseBinary` and `-Version`.                            |

## How it gets built

**Today:** it doesn't. There's no source.

**Once implemented:**

```powershell
# From repo root, after cargo build -p agent --release
./crates/agent/installer/wix/build.ps1 `
  -ReleaseBinary target/release/cmtraceopen-agent.exe `
  -Version 0.1.0
```

CI invokes the same `build.ps1` from `.github/workflows/agent-msi.yml` (also
designed but not yet implemented — see section 10 of the design doc).

## What this directory does NOT own

- **The Intune Cloud PKI cert profile.** That's deployed separately by
  Intune; see [`docs/provisioning/03-intune-cloud-pki.md`](../../../../docs/provisioning/03-intune-cloud-pki.md).
- **The Graph automation that pushes both MSI and cert profile to a device
  group.** Separate Wave 4 deliverable, not yet scoped.
- **Code-signing cert procurement.** See design doc §9 — open question.

## Cross-references

- Design spec: [`docs/wave4/01-msi-design.md`](../../../../docs/wave4/01-msi-design.md)
- Agent crate: [`crates/agent/`](../../)
- Cloud PKI runbook: [`docs/provisioning/03-intune-cloud-pki.md`](../../../../docs/provisioning/03-intune-cloud-pki.md)
