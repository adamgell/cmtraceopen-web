# tools/intune-deploy/ — Wave 4 Intune Graph deployment glue

PowerShell helpers that pack the cmtraceopen-agent MSI into an
Intune `.intunewin` payload and push it (plus its assignment) into a
target device group via the Microsoft Graph API.

This directory is the executable side of
[`docs/provisioning/05-intune-graph-deploy.md`](../../docs/provisioning/05-intune-graph-deploy.md).
Read the runbook first — it explains the full deploy shape (cert profile
→ MSI upload → assignment → device sync → cert lands → MSI installs →
service starts → first bundle ships) and the manual prerequisites that
this code can't automate.

## Scripts

### `Pack-CmtraceAgent.ps1`

Wraps Microsoft's Win32 Content Prep Tool (`IntuneWinAppUtil.exe`).
Auto-downloads the tool to `./.bin/` if it isn't on PATH.

```powershell
pwsh ./Pack-CmtraceAgent.ps1 `
    -SourceFolder 'C:\build\msi-staging' `
    -OutputFolder 'C:\build\out'
```

Parameters:

| Name           | Required | Default                  | Notes                                                            |
| -------------- | -------- | ------------------------ | ---------------------------------------------------------------- |
| `SourceFolder` | yes      | —                        | Folder containing the MSI; all contents get packed.              |
| `SetupFile`    | no       | `CMTraceOpenAgent.msi`   | MSI filename inside `SourceFolder`.                              |
| `OutputFolder` | yes      | —                        | Where the `.intunewin` is written.                               |
| `ToolPath`     | no       | (auto)                   | Override the IntuneWinAppUtil.exe location.                      |
| `Force`        | no       | off                      | Overwrite an existing `.intunewin`.                              |

### `Deploy-CmtraceAgent.ps1`

Authenticates to Microsoft Graph, verifies the device group + Cloud PKI
cert profile assignment, creates a Win32 LOB app entry, uploads the
`.intunewin`, and assigns the app to the target group as `required`.

```powershell
pwsh ./Deploy-CmtraceAgent.ps1 `
    -DeviceGroupName 'cmtraceopen-testdevices' `
    -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
    -MsiProductCode '{12345678-1234-1234-1234-123456789012}' `
    -DryRun
```

Parameters:

| Name              | Required | Default                | Notes                                                                   |
| ----------------- | -------- | ---------------------- | ----------------------------------------------------------------------- |
| `DeviceGroupName` | yes      | —                      | Display name of the target Entra security group.                        |
| `IntuneWinPath`   | yes      | —                      | Output of `Pack-CmtraceAgent.ps1`.                                      |
| `MsiProductCode`  | yes      | —                      | Braced GUID; used for detection rule + uninstall command.               |
| `DisplayName`     | no       | `CMTraceOpen Agent`    | Shown in the Intune portal + Company Portal.                            |
| `Publisher`       | no       | `cmtraceopen`          | Publisher string.                                                       |
| `MsiFileName`     | no       | `CMTraceOpenAgent.msi` | MSI filename inside the `.intunewin`.                                   |
| `TenantId`        | maybe    | —                      | Required for app-only auth.                                             |
| `ClientId`        | maybe    | —                      | Required for app-only auth.                                             |
| `ClientSecret`    | maybe    | —                      | Required for app-only auth.                                             |
| `Supersede`       | no       | off                    | Auto-discover previous apps with the same DisplayName and wire them as superseded. |
| `SupersedesAppIds`| no       | `@()`                  | Explicit supersedence list (mobileApp GUIDs). Overrides `-Supersede`.   |
| `DryRun`          | no       | off                    | Validate everything; skip the create/upload/assign.                     |

Required Graph scopes (for interactive auth):

- `DeviceManagementApps.ReadWrite.All`
- `DeviceManagementConfiguration.ReadWrite.All`
- `GroupMember.Read.All`
- `Group.Read.All`

For app-only auth, grant the same as **Application** permissions on the
Entra app registration with admin consent.

### Upgrades / supersedence

Each MSI build generates a fresh ProductCode, and the Intune Win32 LOB
detection rule is keyed on ProductCode, so every release becomes a new
Intune app. Without any extra glue, the old app stays "installed" on
devices — Intune has no way to know the new app replaces it, and the
rollout silently stalls.

`Deploy-CmtraceAgent.ps1` handles this with `-Supersede`:

```powershell
pwsh ./Deploy-CmtraceAgent.ps1 `
    -DeviceGroupName 'Gell - All Devices' `
    -IntuneWinPath '/tmp/.../CMTraceOpenAgent-0.1.3.intunewin' `
    -MsiProductCode '{NEW-PRODUCT-CODE}' `
    -Supersede
```

What it does:

1. Creates the new app as usual.
2. Queries Graph for other `mobileApps` with the same `DisplayName`.
3. POSTs a `mobileAppSupersedence` relationship from the new app to the
   most-recent prior version (transitive chaining from older releases is
   preserved by Intune, so you don't need to list the whole history).
4. Uses `supersedenceType=update` — MSI-level upgrade via the existing
   `UpgradeCode`, keeps `%ProgramData%\CMTraceOpen\Agent\config.toml`
   untouched. Use `replace` (hand-roll with `-SupersedesAppIds`) if you
   want a full uninstall cycle instead.

On the endpoint, each device's Intune Management Extension picks up the
relationship on its next sync and runs the new MSI. Windows Installer
honors the `MajorUpgrade` policy in `Product.wxs` (after-install-execute,
no downgrade, same-version-allowed), so the service stays up through the
swap and user data survives.

## Prereqs

- PowerShell 7+
- Microsoft.Graph PowerShell SDK (`Install-Module Microsoft.Graph -Scope CurrentUser`)
- An Intune Cloud PKI cert profile already created and assigned to the
  same target group (see
  [`docs/provisioning/03-intune-cloud-pki.md`](../../docs/provisioning/03-intune-cloud-pki.md))
- The agent MSI built by the WiX project (separate PR — does not exist yet)

## Filing issues

Use the repo issue tracker:
<https://github.com/adamgell/cmtraceopen-web/issues>. Tag `wave4` for
deployment-glue bugs and `intune-graph` for issues specific to the Graph
upload flow.
