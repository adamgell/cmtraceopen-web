# 01 — WiX MSI Installer Design (cmtraceopen agent)

**Status:** Design only. No MSI source code in this PR. Implementation lands
in a follow-up once the open questions in section 12 are answered.

**Audience:** anyone implementing the WiX project, plus reviewers who need to
sign off on packaging conventions before code is written.

**Scope:** the `.msi` installer for the `cmtraceopen-agent` Rust binary. Out
of scope: the Intune Cloud PKI cert profile (already provisioned per
[`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md))
and the Graph automation that pushes both the MSI and the cert profile to a
device group (separate Wave 4 deliverable).

---

## 1. Goal

Produce a **signed `CMTraceOpenAgent.msi`** that, when executed on a managed
Windows 10 / 11 device:

1. Drops the agent binary under `%ProgramFiles%\CMTraceOpen\Agent\`.
2. Registers a Windows service named **`CMTraceOpenAgent`**, running as
   `LocalSystem`, start type **automatic-delayed-start**, that launches the
   agent in daemon mode (`cmtraceopen-agent.exe` with no flags — `--oneshot`
   is dev-only).
3. Lays down a default `config.toml` under
   `%ProgramData%\CMTraceOpen\Agent\` that points the agent at the production
   api-server endpoint and tells it to look up its mTLS client cert in
   `LocalMachine\My` by **issuer pattern** (default match: the `Gell - PKI
   Issuing` CA from [`reference_cloud_pki.md`](#) — see section 5).
4. Survives major-upgrade reinstalls without trampling operator config or the
   on-disk upload queue.

The MSI **does not bundle, install, or provision** any client cert. Cert
delivery is the Cloud PKI cert-profile's job; the MSI assumes it is already
in `LocalMachine\My` (or will arrive on the next Intune sync). See section 6
for the soft-warn-on-missing custom action.

The MSI is signed with an Authenticode code-signing cert. The signed binary
inside is also signed. See section 9 for the signing strategy and the
known gap (no cert today).

---

## 2. Layout

Proposed directory tree under `crates/agent/installer/wix/`:

```
crates/agent/installer/wix/
├── README.md                  # stub — this PR; describes layout for future impl
├── Variables.wxi              # version, UpgradeCode, package GUID, install paths
├── Product.wxs                # WiX v4 — Package + MajorUpgrade + Feature graph
├── Files.wxs                  # binary, LICENSE.txt, README.txt
├── Service.wxs                # ServiceInstall + ServiceControl + recovery
├── Config.wxs                 # default config.toml, Queue/, logs/ dirs + ACLs
├── CustomActions/
│   ├── CertCheck.ps1          # OR
│   └── CertCheck/             #   C# DTF project (one or the other, see §6, §12)
└── build.ps1                  # wix.exe wrapper: --release-binary, --version
```

### Why WiX v4 and not v3

- WiX v4 ships as a dotnet tool (`dotnet tool install --global wix`) — no
  more "the WiX Toolset installer + a Visual Studio extension" dance.
- v4 namespace (`http://wixtoolset.org/schemas/v4/wxs`) collapses several v3
  legacy elements (`Product` + `Package` merged into a single `Package`
  element, no more `Wix/Product/Package` triple-nest).
- The schema is stricter — typos in attribute names fail at compile time
  rather than producing a malformed MSI.
- Microsoft's own newer guidance (and the `wix` CLI's docs at
  <https://wixtoolset.org/docs/intro/>) target v4.

### `build.ps1` contract

```powershell
# Build a signed MSI from a release binary.
# Required:
#   -ReleaseBinary  path to cmtraceopen-agent.exe (already built, ideally signed)
#   -Version        semver matching crates/agent/Cargo.toml `version`
# Optional:
#   -SignCertThumbprint  thumbprint of code-signing cert in CurrentUser\My
#                        (skipped if absent; CI sets this)
#   -OutDir              output directory (default: ./out)
#
# Produces: $OutDir\CMTraceOpenAgent-$Version.msi
```

Implementation defers to `wix.exe build` under the hood; the wrapper just
threads parameters through and runs `signtool sign` post-build when a
thumbprint is supplied.

---

## 3. Service installation

The agent is registered as a Windows service via WiX's `ServiceInstall` +
`ServiceControl` elements. Sketch (final form lands in `Service.wxs`):

```xml
<Component Id="AgentServiceInstall" Guid="*">
  <File Id="AgentExe" Source="$(var.ReleaseBinary)" KeyPath="yes" />

  <ServiceInstall
      Id="CMTraceOpenAgentSvc"
      Name="CMTraceOpenAgent"
      DisplayName="CMTrace Open Agent"
      Description="Collects ConfigMgr/Intune/Entra logs and uploads them to the CMTrace Open api-server."
      Type="ownProcess"
      Start="auto"
      ErrorControl="normal"
      Account="LocalSystem"
      Vital="yes">
    <!-- Delayed-start so we don't fight other auto-start services for I/O at boot. -->
    <ServiceConfig DelayedAutoStart="yes" OnInstall="yes" OnReinstall="yes" />
    <!-- Restart the service on crash: 5 min, 5 min, 5 min, then give up.
         Reset failure counter after 1 day. -->
    <util:ServiceConfig
        FirstFailureActionType="restart"
        SecondFailureActionType="restart"
        ThirdFailureActionType="restart"
        RestartServiceDelayInSeconds="300"
        ResetPeriodInDays="1" />
  </ServiceInstall>

  <ServiceControl
      Id="CMTraceOpenAgentCtl"
      Name="CMTraceOpenAgent"
      Start="install"
      Stop="both"
      Remove="uninstall"
      Wait="yes" />
</Component>
```

Notes:

- `Type="ownProcess"` — the agent is its own EXE, not a shared `svchost`
  hosted service.
- `Account="LocalSystem"` — required for full read access to ConfigMgr
  (`C:\Windows\CCM\Logs`), Intune Management Extension
  (`C:\ProgramData\Microsoft\IntuneManagementExtension\Logs`), and Event Log
  channels under `Microsoft-Windows-AAD/*`. A lower-privilege account
  (`NetworkService`) would need ACL grants on each path; LocalSystem keeps
  the install footprint to one component.
- Recovery actions use the WiX util extension (`util:ServiceConfig`). This
  is the "restart 3 times then quit" policy spelled out in the task spec.
- `Stop="both"` — stop the service on both install (so we can replace the
  EXE) and uninstall.
- `Wait="yes"` — block on the SCM until the service confirms state
  transitions, so failures surface in the MSI log instead of silently
  leaving a half-stopped service.

The agent binary itself does not yet implement the Windows service
dispatcher (see TODO at the top of `crates/agent/src/main.rs`). The MSI
spec assumes that work lands first — there's a hard ordering: agent
service-mode → MSI implementation. If service-mode slips, the MSI can ship
in `auto` (non-delayed) mode wrapping the foreground daemon, but the SCM
will then think a clean exit is a crash and hit the recovery actions; not
ideal. Better: gate this PR's implementation on the service dispatcher
work.

---

## 4. Files installed

| Destination                                                 | Source                          | Notes                                                                                  |
| ----------------------------------------------------------- | ------------------------------- | -------------------------------------------------------------------------------------- |
| `%ProgramFiles%\CMTraceOpen\Agent\cmtraceopen-agent.exe`    | `--release-binary` arg          | Signed; KeyPath of the service component.                                              |
| `%ProgramFiles%\CMTraceOpen\Agent\LICENSE.txt`              | repo `LICENSE`                  | MIT license text.                                                                      |
| `%ProgramFiles%\CMTraceOpen\Agent\README.txt`               | crafted at build time           | One-page operator README (where logs go, how to read them, how to reconfigure).        |
| `%ProgramData%\CMTraceOpen\Agent\config.toml`               | `Config.wxs` payload            | Default config; `NeverOverwrite="yes"` + `Permanent="yes"` so operator edits survive.  |
| `%ProgramData%\CMTraceOpen\Agent\Queue\` (directory)        | created empty                   | ACL: `LocalSystem:F`, `Administrators:F`, `Users:R` (read-only for forensics).         |
| `%ProgramData%\CMTraceOpen\Agent\logs\` (directory)         | created empty                   | Same ACL as Queue.                                                                     |

Why `%ProgramData%` and not `%ProgramFiles%` for runtime data: Windows
convention. `%ProgramFiles%` is read-only at runtime (the service runs as
LocalSystem and *could* write there, but Windows Defender ASR rules
sometimes flag program-files writes as suspicious). `%ProgramData%` is the
documented location for per-machine application data and matches what the
agent's own `Queue::default_root()` and config defaults assume today (see
`crates/agent/src/config.rs` doc comments — they explicitly call out
`%ProgramData%\CMTraceOpen\Agent\config.toml`).

ACL detail: granting `Users:R` on the Queue and logs folders lets a local
admin pull a forensic snapshot without elevation. If we want to lock that
down, drop `Users:R` and require `runas` for the audit case. Open question
deferred to section 12.

---

## 5. Default `config.toml`

The MSI lays down this file at `%ProgramData%\CMTraceOpen\Agent\config.toml`
on first install. `NeverOverwrite="yes"` preserves operator edits across
upgrades; uninstall does **not** delete it (matches Windows convention for
operator-mutated config).

```toml
# CMTrace Open Agent — default configuration.
# Lives at %ProgramData%\CMTraceOpen\Agent\config.toml.
# Edits here survive MSI upgrade. To revert to defaults, delete the file
# and re-run the MSI repair (`msiexec /fa CMTraceOpenAgent.msi`).

# Base URL of the api-server. No trailing slash.
# Override per-device with the CMTRACE_API_ENDPOINT env var if you want to
# point one specific device at a non-prod endpoint without touching this file.
api_endpoint = "https://cmtraceopen.corp.example.com"

# HTTP request timeout in seconds. Applies to chunked uploads + control plane
# calls. Bump this if you have slow-link sites; 60 is fine for typical office
# WAN links.
request_timeout_secs = 60

# Cron-like schedule for the evidence collector. Runs once a day at 03:00
# local time by default. Format: standard 5-field cron (min hour dom mon dow).
evidence_schedule = "0 3 * * *"

# Maximum bundles the on-disk upload queue will hold before it starts
# dropping the oldest. Each bundle is roughly 5–50 MB depending on log
# volume; 50 ≈ 250MB–2.5GB worst-case. Tune for your endpoints.
queue_max_bundles = 50

# tracing filter directive. "info" is the right answer for production;
# "debug" is useful for break-glass troubleshooting (expect ~10x log volume).
log_level = "info"

# Device identity. Empty string means "derive from the machine SID at
# runtime" — the agent reads HKLM\SECURITY\SAM\Domains\Account "V" key (or
# falls back to the COMPUTERNAME env var; see resolved_device_id in
# crates/agent/src/config.rs). Set explicitly only if you have a reason to
# pin a specific device_id, e.g. when reprovisioning hardware that should
# inherit the previous device's session history.
device_id = ""

# Log paths the `logs` collector walks for .log / .txt files. Defaults
# cover the ConfigMgr + Intune + DSRegCmd trees. Add custom paths here if
# you want to ship a vendor agent's logs alongside the Microsoft set.
log_paths = [
  "C:\\Windows\\CCM\\Logs\\**\\*.log",
  "C:\\ProgramData\\Microsoft\\IntuneManagementExtension\\Logs\\**\\*.log",
  "C:\\Windows\\Logs\\DSRegCmd\\**\\*.log",
]

# --- Wave 3 mTLS knobs (used once mTLS lands in the agent) ---

[mtls]
# Cert store to search. Always LocalMachine\My for production; agents run as
# LocalSystem and Cloud PKI delivers per-device certs to LocalMachine\My.
cert_store = "LocalMachine\\My"

# Match the cert by issuer Common Name. The Cloud PKI Issuing CA's CN is
# "issuing.gell.internal.cdw.lab" — see reference_cloud_pki.md. We match on
# CN substring rather than full DN so a future CA renewal (with the same CN
# but a new serial) still gets picked up automatically.
issuer_cn_pattern = "issuing.gell.internal.cdw.lab"

# Optional EKU filter. Defaults to clientAuth (1.3.6.1.5.5.7.3.2). Reject any
# cert that doesn't declare clientAuth, even if the issuer matches — defense
# in depth against a code-signing cert from the same CA being used by mistake.
required_eku = "1.3.6.1.5.5.7.3.2"
```

Two notes for reviewers:

1. The `[mtls]` table is forward-looking — the agent's current
   `AgentConfig` struct doesn't read it yet (see `crates/agent/src/config.rs`).
   When the Wave 3 mTLS work lands in the agent itself, it'll consume these
   keys; the MSI shipping them now means the file is already in the right
   shape and operators don't have to edit a second time on the mTLS bump.
2. `device_id = ""` matches the `from_env_or_default` precedence: empty →
   derive at runtime. The agent today falls back to `COMPUTERNAME` (see
   `resolved_device_id` in `config.rs`); the machine-SID derivation is a
   small follow-up that doesn't block the MSI.

---

## 6. Cert-lookup custom action

**Goal:** at install time, log a warning if the Cloud PKI client cert is not
yet in `LocalMachine\My`. Do **not** fail the install — the cert may arrive
on the next Intune sync (typical lag: minutes to a few hours), and a
hard-fail would block fleet rollouts that interleave MSI deploy + cert
profile assignment.

### Two implementation options

**Option A: PowerShell deferred custom action**

- Pros: trivial to write (~15 lines of PS), zero compile step, no extra
  toolchain in the build.
- Cons: depends on `powershell.exe` being available (it is, on every
  supported Windows SKU including Server Core), and risks tripping over
  group-policy ExecutionPolicy. The mitigation is `-ExecutionPolicy Bypass`
  on the invocation — bypass at the per-script level is allowed even when
  Restricted is enforced machine-wide, because MSI custom actions run
  outside the ExecutionPolicy enforcement context (`Bypass` literally
  exempts the script). Still, AV / EDR may flag a `-Bypass` from an MSI
  custom action; some shops will see SmartScreen / Defender alerts.

Sketch:

```powershell
# CertCheck.ps1 — runs as a deferred MSI custom action. STDOUT/STDERR are
# captured to the MSI log via the WiX wrapper. Always exits 0 — see the
# soft-warn rationale in the design doc.
$pattern = 'issuing.gell.internal.cdw.lab'
$found = Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.Issuer -match $pattern }

if ($found) {
  Write-Output "OK: found $($found.Count) Cloud PKI cert(s) matching issuer '$pattern'."
} else {
  Write-Output "WARN: no client cert matching issuer '$pattern' in LocalMachine\My."
  Write-Output "WARN: agent will start but mTLS calls to api-server will fail until the cert arrives."
  Write-Output "WARN: confirm the Intune Cloud PKI cert profile is assigned to this device."
}
exit 0
```

**Option B: C# DTF custom action (compiled native DLL)**

- Pros: no script-policy entanglements, runs entirely in-process under
  msiexec; harder for AV to flag (it's a signed DLL, not a .ps1). Supports
  rich MSI logging via `session.Log()`. Deterministic, no shell-out.
- Cons: needs a small C# project + the `WixToolset.Dtf.WindowsInstaller`
  NuGet package; extra build step (`dotnet build -c Release`); the DLL
  has to be signed too if we want a fully-signed package.

Sketch (just the public method body):

```csharp
[CustomAction]
public static ActionResult VerifyCloudPkiCert(Session session) {
    var pattern = "issuing.gell.internal.cdw.lab";
    using var store = new X509Store(StoreName.My, StoreLocation.LocalMachine);
    store.Open(OpenFlags.ReadOnly);
    var matches = store.Certificates
        .Cast<X509Certificate2>()
        .Where(c => c.Issuer.IndexOf(pattern, StringComparison.OrdinalIgnoreCase) >= 0)
        .ToList();
    if (matches.Count == 0) {
        session.Log("WARN: no Cloud PKI cert matching issuer '{0}' in LocalMachine\\My", pattern);
    } else {
        session.Log("OK: found {0} Cloud PKI cert(s) matching issuer '{1}'", matches.Count, pattern);
    }
    return ActionResult.Success; // never fail
}
```

**Recommendation:** start with Option A (PowerShell) for the first MSI
release. Move to Option B if Defender/SmartScreen complaints from pilot
deployments make the PS path painful. Either way, the action runs
**before `StartServices`** so the warning lands in the MSI log next to the
service-start step where an operator triaging "service is up but uploads
fail" will see it.

Open question deferred to section 12: which option to implement first.

---

## 7. Upgrade behavior

Major-upgrade chain, expressed via WiX's `MajorUpgrade` element:

```xml
<MajorUpgrade
    Schedule="afterInstallExecute"
    AllowSameVersionUpgrades="yes"
    DowngradeErrorMessage="A newer version of CMTrace Open Agent is already installed."
    AllowDowngrades="no" />
```

Behavior:

- **`Schedule="afterInstallExecute"`** — uninstall the old version *after*
  the new one's files are laid down. Avoids a brief service-down window
  during the upgrade and lets MSI deduplicate identical files (so we don't
  pointlessly stop+start the service for an unchanged binary).
- **`AllowSameVersionUpgrades="yes"`** — `msiexec /i` against the same
  version reinstalls cleanly. Useful for repair-style operator workflows
  and for CI smoke tests that install the same MSI twice.
- **`AllowDowngrades="no"`** — refuse to install version 1.2 over 1.3.
  Operators who actually need to roll back uninstall first.

Preservation across upgrades:

- `config.toml` — `NeverOverwrite="yes"` + `Permanent="yes"` on its
  component. Survives.
- `Queue/` and `logs/` directories — not owned by any component
  (`CreateFolder` only with no file payload), so MSI never touches their
  contents. Survives.
- Binary, LICENSE, README — overwritten with the new version's payload.
  Standard MSI file-versioning rules; the binary's `FILEVERSION` resource
  (set by the Rust build via a `build.rs` or by `cargo-wix`-style metadata)
  drives the comparison.

---

## 8. Uninstall

Default uninstall behavior:

1. Stop the `CMTraceOpenAgent` service (`ServiceControl Stop="both"`).
2. Remove the service registration.
3. Delete `%ProgramFiles%\CMTraceOpen\Agent\` (binary, LICENSE, README).
4. **Leave** `%ProgramData%\CMTraceOpen\Agent\` intact — config.toml,
   Queue/, logs/ all stay. Matches Windows convention: user / operator
   data is not nuked by an uninstall.

Operators who want a full purge set the MSI property `KEEP_USER_DATA=0`:

```cmd
msiexec /x CMTraceOpenAgent.msi KEEP_USER_DATA=0 /qn
```

When `KEEP_USER_DATA=0`, a `RemoveFolderEx` action under
`InstallExecuteSequence` deletes `%ProgramData%\CMTraceOpen\Agent\` recursively.
Default value (set in `Property` element):

```xml
<Property Id="KEEP_USER_DATA" Value="1" />
```

The condition on the `RemoveFolderEx` action:

```xml
<Custom Action="PurgeUserData" After="RemoveFiles">
  KEEP_USER_DATA = "0" AND REMOVE = "ALL"
</Custom>
```

`REMOVE="ALL"` ensures we only run on uninstall, not on upgrade-driven
component removal.

---

## 9. Code signing strategy

Two artifacts need signing:

1. The `cmtraceopen-agent.exe` binary inside the MSI.
2. The `CMTraceOpenAgent.msi` file itself.

Both signed with the same Authenticode code-signing cert.

### The gap

**We do not have a code-signing cert today.** The MSI design assumes one
will be acquired before the first signed release. Until then the MSI can
ship unsigned (Intune Win32 LOB doesn't strictly require a signed MSI), but
operators will see SmartScreen "unrecognized publisher" warnings and the
binary will be more likely to trip Defender heuristic detections.

### Modern path — Azure Trusted Signing (recommended)

- <https://learn.microsoft.com/azure/trusted-signing/>
- Cert lives in Azure, never on a build agent's disk. Signing is a Graph /
  REST call; signtool integrates via `Azure.CodeSigning.Dlib`.
- ~$10/month per identity — far cheaper than EV certs from traditional CAs.
- Reputation is shared across all Trusted Signing customers, so SmartScreen
  trust accrues faster than for a brand-new private cert.
- Identity validation is one-time per Microsoft Partner Center org —
  similar to Apple Developer ID enrollment.

### Legacy path — DigiCert / GlobalSign EV cert

- ~$300–700/year, hardware token (USB HSM) required.
- Faster SmartScreen trust accrual than non-EV (EV gets immediate
  reputation), but token-on-build-agent is a CI nightmare. Workarounds
  exist (DigiCert KeyLocker, GlobalSign Atlas) but they cost extra and
  rebuild the Trusted-Signing model less elegantly.

**Recommendation:** Trusted Signing. If procurement blocks Azure
subscription bumps, fallback is to ship unsigned during pilot and acquire
a DigiCert EV cert before GA.

### Why sign the MSI when Intune doesn't require it

- SmartScreen: a signed MSI from a trusted publisher gets immediate "open"
  in the SmartScreen prompt; unsigned gets the scary red dialog.
- Supply chain: a signed MSI lets downstream systems (the Graph automation
  in Wave 4, future Group Policy deployment) verify provenance before
  running the installer. Unsigned MSIs are a known supply-chain attack
  vector — if an attacker swaps the MSI in flight, the signature breaks
  loudly.
- Defender ASR rules sometimes block unsigned MSIs from running under
  service contexts; signing avoids the gotcha.

### What goes in `build.ps1`

```powershell
# Build the MSI.
wix build -arch x64 -out "$OutDir\CMTraceOpenAgent-$Version.msi" `
  -d ReleaseBinary="$ReleaseBinary" -d Version="$Version" `
  Variables.wxi Product.wxs Files.wxs Service.wxs Config.wxs

# Sign it (only if a thumbprint was provided).
if ($SignCertThumbprint) {
  & signtool sign /sha1 $SignCertThumbprint /tr http://timestamp.digicert.com `
    /td sha256 /fd sha256 /a "$OutDir\CMTraceOpenAgent-$Version.msi"
}
```

(The exe inside the MSI gets signed earlier in CI, before WiX picks it up
— see section 10.)

> See `02a-sign-every-component.md` for the per-artifact signing flow (every EXE / DLL / PS / MSI gets signed in the right order).

---

## 10. CI build

Proposed workflow at `.github/workflows/agent-msi.yml`. Design only — no
implementation in this PR.

```yaml
name: agent-msi
on:
  push:
    tags: ['agent-v*']
  workflow_dispatch:
    inputs:
      version:
        description: 'semver to build (omit to use Cargo.toml)'
        required: false

jobs:
  build:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.90
      - name: Build agent (release)
        run: cargo build -p agent --release --target x86_64-pc-windows-msvc
      - name: Sign agent EXE
        if: ${{ secrets.AZURE_TRUSTED_SIGNING_CONFIG != '' }}
        run: |
          # signtool with Azure Trusted Signing dlib — see ts docs.
          # Skipped on PR builds (secrets aren't exposed); skipped until cert exists.
        env:
          AZURE_CONFIG: ${{ secrets.AZURE_TRUSTED_SIGNING_CONFIG }}
      - name: Install WiX v4
        run: dotnet tool install --global wix
      - name: Build MSI
        run: |
          $version = '${{ github.event.inputs.version }}'
          if (-not $version) {
            $version = (Select-String '^version' crates/agent/Cargo.toml | Select-Object -First 1).Line.Split('"')[1]
          }
          ./crates/agent/installer/wix/build.ps1 `
            -ReleaseBinary target/x86_64-pc-windows-msvc/release/cmtraceopen-agent.exe `
            -Version $version
      - name: Sign MSI
        if: ${{ secrets.AZURE_TRUSTED_SIGNING_CONFIG != '' }}
        run: ./crates/agent/installer/wix/build.ps1 -SignOnly -Version $version
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: CMTraceOpenAgent-msi
          path: out/*.msi
      - name: Attach to release
        if: startsWith(github.ref, 'refs/tags/agent-v')
        uses: softprops/action-gh-release@v2
        with:
          files: out/*.msi
```

Trigger contract: `git tag agent-v0.1.0 && git push --tags`. The
api-server has its own release tag namespace (`api-v*`); agent uses
`agent-v*` to keep the two trains independent.

Until the signing cert is procured, the workflow runs the unsigned path
and uploads an unsigned MSI as the release artifact. That's fine for
internal pilot but should not ship to external operators.

---

## 11. Versioning

- **Authoritative source:** `crates/agent/Cargo.toml` `version`. Today
  it's `0.1.0`. Bumping the agent crate is the only place a human edits
  the version.
- **MSI `ProductVersion`:** mirrors the Cargo version. `build.ps1` reads
  the Cargo.toml at build time (or accepts an override `-Version` for CI
  pre-release builds).
- **MSI `UpgradeCode`:** **fixed forever**. Generated once for this PR:

  ```
  463FD20A-1029-448F-AE5B-F81C818861D0
  ```

  This GUID is documented in `Variables.wxi` and **must never change**.
  Changing it breaks the upgrade chain — old installs become orphaned and
  must be uninstalled by hand before the new MSI will install. Reviewers:
  confirm this GUID is unique against any prior cmtraceopen artifacts
  before merge (it should be — first MSI we've ever shipped).

- **MSI `ProductCode`:** auto-generated per build via `Product/@Id="*"`.
  This is the per-version GUID; each release gets a fresh one and the
  `MajorUpgrade` element uses it to detect "is the installed version
  different from the one I'm trying to install."

- **Package `Id`:** auto-generated each build (WiX v4 default behavior).
  The `Package/@Id` is per-MSI-file, distinct from `ProductCode`.

Compatibility matrix:

| Cargo `version` | MSI `ProductVersion` | UpgradeCode                          | ProductCode |
| --------------- | -------------------- | ------------------------------------ | ----------- |
| 0.1.0           | 0.1.0.0              | 463FD20A-1029-448F-AE5B-F81C818861D0 | auto        |
| 0.1.1           | 0.1.1.0              | 463FD20A-1029-448F-AE5B-F81C818861D0 | auto        |
| 0.2.0           | 0.2.0.0              | 463FD20A-1029-448F-AE5B-F81C818861D0 | auto        |
| 1.0.0           | 1.0.0.0              | 463FD20A-1029-448F-AE5B-F81C818861D0 | auto        |

Note: MSI `ProductVersion` is a four-part `Major.Minor.Build.Revision`
where only the first three parts are compared for upgrade detection. Map
semver `0.1.1` → MSI `0.1.1.0`. We don't use the Revision slot today; if
we ever ship a hotfix that's a strict superset of the prior release, we
could bump `Revision` instead of `Patch` to keep semver clean.

---

## 12. Open questions (decide before implementation)

1. **Code-signing cert source.** Azure Trusted Signing (recommended) vs
   DigiCert/GlobalSign EV cert vs ship unsigned for pilot? Procurement
   path matters; Trusted Signing requires an Azure subscription bump and
   Microsoft Partner Center identity verification.

2. **Custom action language.** PowerShell (`CertCheck.ps1`, easy, may
   trip AV) vs C# DTF (`CertCheck.dll`, more robust, extra build step)
   for the Cloud PKI cert presence check?

3. **Day-1 silent install + MST overrides.** Do we want to support
   shipping a `.mst` transform alongside the MSI so operators can override
   `api_endpoint` / `device_id` at install time without touching
   config.toml? Adds ~half a day of WiX work and an Orca-or-equivalent
   dependency for ops; punt unless there's pilot pressure.

4. **`Users:R` on the Queue/ and logs/ folders.** Yes (forensic
   convenience) or no (least privilege)? Default in the spec is `Users:R`
   — flip to admin-only if security review pushes back.

5. **Service dispatcher in the agent.** This MSI design assumes
   `crates/agent/src/main.rs` grows a real `windows_service::service_dispatcher`
   integration before the MSI ships. Confirm ordering: agent service
   support → MSI work → CI workflow. If the order flips, the recovery
   actions in section 3 will fire on every clean exit and look like a
   crash loop.

6. **`device_id` derivation from machine SID.** The spec mentions this
   for the default config but the agent doesn't implement it yet (it
   falls back to `COMPUTERNAME`). Either land the SID derivation in the
   agent first, or change the default config comment to admit "today
   defaults to COMPUTERNAME, will move to machine SID in a follow-up."

---

## Implementation gating

Per the open questions above, the implementation PR is blocked on:

- (Required) Question 1: signing strategy. Even an "unsigned for pilot"
  decision unblocks; it's the lack of a decision that blocks.
- (Required) Question 2: PS vs DTF. Either works; pick one before the WiX
  source goes in.
- (Required) Question 5: confirm the agent's service dispatcher work
  lands first or accept the temporary crash-loop appearance.
- (Optional) Questions 3, 4, 6 can be answered post-merge of the
  implementation; defaults in the spec are reasonable starting points.
