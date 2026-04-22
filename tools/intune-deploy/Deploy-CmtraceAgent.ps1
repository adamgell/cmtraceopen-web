<#
.SYNOPSIS
    Deploys the cmtraceopen-agent MSI to an Intune-managed Windows device group via Microsoft Graph.

.DESCRIPTION
    Wave 4 deployment automation. This script orchestrates the Win32 LOB upload flow:
        1. Authenticates to Microsoft Graph (interactive or app-only).
        2. Verifies the target Entra device group exists.
        3. Verifies an Intune Cloud PKI cert profile is already assigned to that group
           (warns if not — agent without cert is useless on Wave 3+).
        4. Creates a Win32 LOB app entry pointing at the supplied .intunewin payload.
        5. Creates a content version, requests an Azure-blob SAS URL from Graph, uploads
           the encrypted payload in 6 MiB chunks, and commits the file.
        6. Assigns the app to the target device group as 'required'.
        7. Prints a summary (app id, content version, assignment count, portal URL).

.PARAMETER DeviceGroupName
    Display name of the Entra device group to deploy to. Must exist before this runs.
    Example: 'cmtraceopen-testdevices' (per docs/provisioning/03-intune-cloud-pki.md).

.PARAMETER IntuneWinPath
    Absolute path to the .intunewin payload built by Pack-CmtraceAgent.ps1.

.PARAMETER DisplayName
    App display name shown in the Intune portal and Company Portal. Default: 'CMTraceOpen Agent'.

.PARAMETER Publisher
    App publisher string. Default: 'cmtraceopen'.

.PARAMETER MsiProductCode
    The MSI ProductCode GUID (curly braces required). Used for the detection rule and
    uninstall command line. The WiX MSI build is expected to emit this; until the MSI
    exists, pass any well-formed GUID for -DryRun validation.

.PARAMETER MsiFileName
    The MSI file name inside the .intunewin payload. Default: 'CMTraceOpenAgent.msi'.

.PARAMETER TenantId
    Entra tenant ID. Required when using -ClientId / -ClientSecret (app-only auth).

.PARAMETER ClientId
    Entra app registration (client) ID for app-only auth. Optional.

.PARAMETER ClientSecret
    Client secret for app-only auth. Optional. Pair with -ClientId / -TenantId.

.PARAMETER DryRun
    Validate credentials, group existence, cert-profile assignment, and payload presence.
    Skip the actual app create / upload / assign calls.

.NOTES
    Prereqs:
      * PowerShell 7+
      * Microsoft.Graph PowerShell SDK installed:
            Install-Module Microsoft.Graph -Scope CurrentUser
      * The Intune Cloud PKI cert profile from docs/provisioning/03-intune-cloud-pki.md
        already created and assigned to the same -DeviceGroupName.
      * The .intunewin payload built by Pack-CmtraceAgent.ps1 (which itself depends on
        the WiX MSI build — separate PR).

    Required Graph scopes (interactive auth requests these):
      * DeviceManagementApps.ReadWrite.All
      * DeviceManagementConfiguration.ReadWrite.All
      * GroupMember.Read.All
      * Group.Read.All

    For app-only auth, the same permissions must be granted to the app registration as
    Application permissions (not Delegated), with admin consent.

.EXAMPLE
    pwsh ./Deploy-CmtraceAgent.ps1 `
        -DeviceGroupName 'cmtraceopen-testdevices' `
        -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
        -MsiProductCode '{12345678-1234-1234-1234-123456789012}' `
        -DryRun

.EXAMPLE
    pwsh ./Deploy-CmtraceAgent.ps1 `
        -DeviceGroupName 'cmtraceopen-testdevices' `
        -IntuneWinPath 'C:\build\out\CMTraceOpenAgent.intunewin' `
        -MsiProductCode '{12345678-1234-1234-1234-123456789012}' `
        -TenantId '00000000-0000-0000-0000-000000000000' `
        -ClientId '11111111-1111-1111-1111-111111111111' `
        -ClientSecret $env:CMTRACE_GRAPH_SECRET

.LINK
    https://learn.microsoft.com/mem/intune/apps/apps-add-graph-api
.LINK
    https://learn.microsoft.com/mem/intune/protect/microsoft-cloud-pki-overview
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DeviceGroupName,

    [Parameter(Mandatory = $true)]
    [string]$IntuneWinPath,

    [string]$DisplayName = 'CMTraceOpen Agent',

    [string]$Publisher = 'cmtraceopen',

    [Parameter(Mandatory = $true)]
    [string]$MsiProductCode,

    [string]$MsiFileName = 'CMTraceOpenAgent.msi',

    [string]$TenantId,

    [string]$ClientId,

    [string]$ClientSecret,

    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScopesInteractive = @(
    'DeviceManagementApps.ReadWrite.All',
    'DeviceManagementConfiguration.ReadWrite.All',
    'GroupMember.Read.All',
    'Group.Read.All'
)

$ChunkSize = 6 * 1024 * 1024  # 6 MiB — Microsoft-documented max chunk for Win32 LOB upload.

function Write-Section {
    param([string]$Message)
    Write-Host ''
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Test-PreReqs {
    if ($PSVersionTable.PSVersion.Major -lt 7) {
        throw "PowerShell 7+ required. Detected: $($PSVersionTable.PSVersion)"
    }

    $required = @(
        'Microsoft.Graph.Authentication',
        'Microsoft.Graph.Groups',
        'Microsoft.Graph.Devices.CorporateManagement',
        'Microsoft.Graph.DeviceManagement'
    )
    foreach ($mod in $required) {
        if (-not (Get-Module -ListAvailable -Name $mod)) {
            throw "Missing required module: $mod. Install with: Install-Module Microsoft.Graph -Scope CurrentUser"
        }
        Import-Module $mod -ErrorAction Stop | Out-Null
    }

    if (-not (Test-Path -LiteralPath $IntuneWinPath)) {
        throw "IntuneWinPath not found: $IntuneWinPath"
    }
    if (-not $IntuneWinPath.ToLowerInvariant().EndsWith('.intunewin')) {
        throw "IntuneWinPath must end in .intunewin: $IntuneWinPath"
    }

    if ($MsiProductCode -notmatch '^\{[0-9A-Fa-f-]{36}\}$') {
        throw "MsiProductCode must be a braced GUID like '{12345678-1234-1234-1234-123456789012}'."
    }
}

function Connect-Graph {
    if ($ClientId -and $ClientSecret -and $TenantId) {
        Write-Section "Connecting to Graph (app-only) tenant=$TenantId clientId=$ClientId"
        $secure = ConvertTo-SecureString $ClientSecret -AsPlainText -Force
        $cred = [pscredential]::new($ClientId, $secure)
        Connect-MgGraph -TenantId $TenantId -ClientSecretCredential $cred -NoWelcome
    }
    elseif ($ClientId -or $ClientSecret) {
        throw "App-only auth requires all three of -TenantId, -ClientId, -ClientSecret."
    }
    else {
        Write-Section "Connecting to Graph (interactive) scopes=$($ScopesInteractive -join ',')"
        Connect-MgGraph -Scopes $ScopesInteractive -NoWelcome
    }

    $ctx = Get-MgContext
    if (-not $ctx) { throw "Connect-MgGraph returned no context." }
    Write-Host "  authenticated as: $($ctx.Account)  tenant: $($ctx.TenantId)" -ForegroundColor Gray
}

function Get-DeviceGroup {
    Write-Section "Resolving device group '$DeviceGroupName'"
    $filter = "displayName eq '" + ($DeviceGroupName -replace "'", "''") + "'"
    $groups = Get-MgGroup -Filter $filter -ConsistencyLevel eventual -CountVariable count -ErrorAction Stop
    if (-not $groups -or $groups.Count -eq 0) {
        throw "Device group not found: $DeviceGroupName"
    }
    if ($groups.Count -gt 1) {
        throw "Ambiguous device group name '$DeviceGroupName' — $($groups.Count) matches."
    }
    Write-Host "  found group id: $($groups[0].Id)" -ForegroundColor Gray
    return $groups[0]
}

function Test-CertProfileAssignment {
    param([Parameter(Mandatory)] $Group)

    Write-Section "Checking that a Cloud PKI cert profile is assigned to this group"
    # Cloud PKI cert profiles surface as deviceConfigurations of type
    # windowsDomainJoinConfiguration / windows81SCEPCertificateProfile /
    # windows81PfxImportCertificateProfile / windowsPkcsCertificateProfile in Graph.
    # We don't dictate which — just check that *some* cert-related config targets the group.
    try {
        $configs = Get-MgDeviceManagementDeviceConfiguration -All -ErrorAction Stop
    }
    catch {
        Write-Warning "Could not enumerate deviceConfigurations: $($_.Exception.Message)"
        Write-Warning "Cert-profile assignment NOT verified. Continuing — but if no Cloud PKI cert"
        Write-Warning "profile is assigned to '$($Group.DisplayName)', the deployed agent will fail mTLS."
        return
    }

    $certKeywords = @('SCEP', 'Pkcs', 'Certificate', 'PFX', 'CloudPki')
    $candidates = $configs | Where-Object {
        $type = $_.AdditionalProperties['@odata.type']
        if (-not $type) { return $false }
        foreach ($kw in $certKeywords) { if ($type -match $kw) { return $true } }
        return $false
    }

    if (-not $candidates -or $candidates.Count -eq 0) {
        Write-Warning "No cert-profile-shaped device configurations found in the tenant."
        Write-Warning "Run docs/provisioning/03-intune-cloud-pki.md before deploying the agent."
        return
    }

    $matched = $false
    foreach ($cfg in $candidates) {
        try {
            $assignments = Get-MgDeviceManagementDeviceConfigurationAssignment `
                -DeviceConfigurationId $cfg.Id -ErrorAction Stop
        }
        catch { continue }

        foreach ($a in $assignments) {
            $target = $a.Target.AdditionalProperties
            if ($target -and $target['groupId'] -eq $Group.Id) {
                Write-Host "  cert profile '$($cfg.DisplayName)' is assigned to '$($Group.DisplayName)'" -ForegroundColor Green
                $matched = $true
                break
            }
        }
        if ($matched) { break }
    }

    if (-not $matched) {
        Write-Warning "No Cloud PKI cert profile is assigned to '$($Group.DisplayName)'."
        Write-Warning "The agent will install but cannot mTLS-auth to api-server until the profile is assigned."
        Write-Warning "See docs/provisioning/03-intune-cloud-pki.md Step 4."
    }
}

function New-Win32LobAppPayload {
    param(
        [Parameter(Mandatory)] [string]$AppDisplayName,
        [Parameter(Mandatory)] [string]$AppPublisher,
        [Parameter(Mandatory)] [string]$ProductCode,
        [Parameter(Mandatory)] [string]$SetupFile
    )

    $detectionRule = @{
        '@odata.type'   = '#microsoft.graph.win32LobAppProductCodeDetection'
        productCode     = $ProductCode
        productVersion  = $null
        productVersionOperator = 'notConfigured'
    }

    return @{
        '@odata.type'                      = '#microsoft.graph.win32LobApp'
        displayName                        = $AppDisplayName
        description                        = 'cmtraceopen-agent — Windows service that ships log bundles to the api-server. Deployed by tools/intune-deploy/Deploy-CmtraceAgent.ps1 (Wave 4).'
        publisher                          = $AppPublisher
        fileName                           = [System.IO.Path]::GetFileName($IntuneWinPath)
        setupFilePath                      = $SetupFile
        installCommandLine                 = "msiexec /i `"$SetupFile`" /qn"
        uninstallCommandLine               = "msiexec /x $ProductCode /qn"
        applicableArchitectures            = 'x64'
        minimumSupportedOperatingSystem    = @{
            '@odata.type' = '#microsoft.graph.windowsMinimumOperatingSystem'
            v10_1903      = $true
        }
        installExperience                  = @{
            '@odata.type'          = '#microsoft.graph.win32LobAppInstallExperience'
            runAsAccount           = 'system'
            deviceRestartBehavior  = 'suppress'
        }
        detectionRules                     = @($detectionRule)
        msiInformation                     = @{
            '@odata.type'    = '#microsoft.graph.win32LobAppMsiInformation'
            productCode      = $ProductCode
            productVersion   = '0.1.0'
            upgradeCode      = $ProductCode
            requiresReboot   = $false
            packageType      = 'perMachine'
            productName      = $AppDisplayName
            publisher        = $AppPublisher
        }
    }
}

function Invoke-IntuneWinUpload {
    param(
        [Parameter(Mandatory)] [string]$AppId,
        [Parameter(Mandatory)] [string]$IntuneWinPath
    )

    # Win32 LOB upload flow per https://learn.microsoft.com/mem/intune/apps/apps-add-graph-api
    #   1. POST .../mobileApps/{id}/microsoft.graph.win32LobApp/contentVersions   → version id
    #   2. POST .../contentVersions/{vid}/files                                    → file entry (placeholder)
    #   3. GET  .../files/{fid} until uploadState == 'azureStorageUriRequestSuccess'
    #   4. PUT chunks to the returned azureStorageUri (SAS URL) as block blobs
    #   5. POST .../files/{fid}/commit  with the encryption info
    #   6. PATCH .../mobileApps/{id} with committedContentVersion = {vid}
    #
    # Step 4 uses Invoke-RestMethod against the SAS URL — NOT a Graph endpoint.
    # The .intunewin file is already-encrypted by IntuneWinAppUtil.exe; the encryption
    # info needed for the commit comes from Detection.xml inside the .intunewin (which
    # is itself a renamed zip). Pack-CmtraceAgent.ps1 produces this; we just relay it.

    Write-Section "Win32 LOB upload — would execute the 6-step Graph + Azure blob flow"
    Write-Host "  app id      : $AppId" -ForegroundColor Gray
    Write-Host "  payload     : $IntuneWinPath" -ForegroundColor Gray
    Write-Host "  chunk size  : $ChunkSize bytes" -ForegroundColor Gray

    if ($DryRun) {
        Write-Host "  [DryRun] skipping content upload" -ForegroundColor Yellow
        return [pscustomobject]@{ ContentVersionId = '(dry-run)'; FileId = '(dry-run)' }
    }

    # NOTE: Full implementation pending. The Graph SDK does not yet wrap the
    # contentVersions / files / commit endpoints, so this is a thin Invoke-MgGraphRequest
    # + Invoke-RestMethod flow. Tracked alongside the WiX MSI work; this scaffold gives
    # the orchestration shape so reviewers can see the contract.
    throw "Live upload path not yet implemented. Re-run with -DryRun until the WiX MSI + upload helper land."
}

function New-AppAssignment {
    param(
        [Parameter(Mandatory)] [string]$AppId,
        [Parameter(Mandatory)] [string]$GroupId
    )

    Write-Section "Assigning app to group as 'required'"
    if ($DryRun) {
        Write-Host "  [DryRun] skipping assignment create" -ForegroundColor Yellow
        return $null
    }

    $body = @{
        '@odata.type' = '#microsoft.graph.mobileAppAssignment'
        intent        = 'required'
        target        = @{
            '@odata.type' = '#microsoft.graph.groupAssignmentTarget'
            groupId       = $GroupId
        }
        settings      = @{
            '@odata.type'                = '#microsoft.graph.win32LobAppAssignmentSettings'
            notifications                = 'showAll'
            deliveryOptimizationPriority = 'notConfigured'
            installTimeSettings          = $null
            restartSettings              = $null
        }
    }
    $uri = "https://graph.microsoft.com/beta/deviceAppManagement/mobileApps/$AppId/assignments"
    return Invoke-MgGraphRequest -Method POST -Uri $uri -Body ($body | ConvertTo-Json -Depth 8)
}

# ---------- main ----------

Test-PreReqs

Connect-Graph
$group = Get-DeviceGroup
Test-CertProfileAssignment -Group $group

$payload = New-Win32LobAppPayload `
    -AppDisplayName $DisplayName `
    -AppPublisher   $Publisher `
    -ProductCode    $MsiProductCode `
    -SetupFile      $MsiFileName

Write-Section "Creating Win32 LOB app entry"
if ($DryRun) {
    Write-Host "  [DryRun] would POST mobileApps with body:" -ForegroundColor Yellow
    $payload | ConvertTo-Json -Depth 8 | Write-Host
    $appId = '(dry-run-app-id)'
}
else {
    $created = Invoke-MgGraphRequest -Method POST `
        -Uri 'https://graph.microsoft.com/beta/deviceAppManagement/mobileApps' `
        -Body ($payload | ConvertTo-Json -Depth 8)
    $appId = $created.id
    Write-Host "  app id: $appId" -ForegroundColor Gray
}

$content = Invoke-IntuneWinUpload -AppId $appId -IntuneWinPath $IntuneWinPath

$assignment = New-AppAssignment -AppId $appId -GroupId $group.Id

Write-Section 'Summary'
$portalUrl = if ($DryRun) {
    'https://intune.microsoft.com/#view/Microsoft_Intune_Apps/SettingsMenu/~/0/appId/(dry-run)'
} else {
    "https://intune.microsoft.com/#view/Microsoft_Intune_Apps/SettingsMenu/~/0/appId/$appId"
}
[pscustomobject]@{
    AppId            = $appId
    DisplayName      = $DisplayName
    DeviceGroup      = $group.DisplayName
    DeviceGroupId    = $group.Id
    ContentVersionId = $content.ContentVersionId
    AssignmentCount  = if ($assignment) { 1 } else { 0 }
    DryRun           = [bool]$DryRun
    PortalUrl        = $portalUrl
} | Format-List

Write-Host ''
Write-Host 'Done. Devices in the target group will pick up the app on their next Intune sync (5–30 min).' -ForegroundColor Green
