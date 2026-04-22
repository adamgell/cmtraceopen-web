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
| `DryRun`          | no       | off                    | Validate everything; skip the create/upload/assign.                     |

Required Graph scopes (for interactive auth):

- `DeviceManagementApps.ReadWrite.All`
- `DeviceManagementConfiguration.ReadWrite.All`
- `GroupMember.Read.All`
- `Group.Read.All`

For app-only auth, grant the same as **Application** permissions on the
Entra app registration with admin consent.

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
