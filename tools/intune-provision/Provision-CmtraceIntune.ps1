<#
.SYNOPSIS
    Wires up the cmtraceopen Intune provisioning around an existing
    Cloud PKI SCEP cert profile and trusted-root device configuration
    that were created in the Intune portal.

.DESCRIPTION
    This is the "Path B" flow: the SCEP cert profile already exists in
    the portal (default name `Gell - SCEP Cert`) and covers both the
    Client Authentication and Code Signing EKUs. Likewise the Cloud PKI
    root cert is already published as a trusted-root device config
    (default name `Gell - Root Trusted Cert`).

    What this script does, via Microsoft Graph:

        1. Creates (or reuses) two Entra security groups:
               cmtraceopen-testdevices    (agent hosts; will grow)
               cmtraceopen-build-machines (code-signing runner hosts)

        2. Looks up the existing SCEP cert profile and trusted-root
           device config by display name.

        3. Assigns BOTH existing configs to BOTH groups. Because the
           combined SCEP profile issues a cert with both EKUs, either
           group membership is sufficient to pick up a fully-useful cert
           on the device.

        4. Prints a summary + a reminder to confirm the SAN URI on the
           SCEP profile (required by the api-server's mTLS identity
           parser — path B asks you to add this in the portal once).

    Idempotent: safe to rerun.

.PARAMETER SharedCertProfileName
    Display name of the existing Cloud PKI SCEP cert profile.
    Default: 'Gell - SCEP Cert'.

.PARAMETER TrustedRootConfigName
    Display name of the existing trusted-root device config wrapping the
    Cloud PKI root CA. Default: 'Gell - Root Trusted Cert'.

.PARAMETER TestDeviceGroupName
    Default: cmtraceopen-testdevices.

.PARAMETER BuildMachineGroupName
    Default: cmtraceopen-build-machines.

.PARAMETER TenantId
    Optional. If omitted, the tenant from Connect-MgGraph is used.

.EXAMPLE
    pwsh ./tools/intune-provision/Provision-CmtraceIntune.ps1

    Uses all defaults — expects 'Gell - SCEP Cert' and
    'Gell - Root Trusted Cert' to already exist in the tenant.

.EXAMPLE
    pwsh ./tools/intune-provision/Provision-CmtraceIntune.ps1 `
        -SharedCertProfileName 'My SCEP profile' `
        -TrustedRootConfigName 'My root trust'

.NOTES
    Required Graph delegated scopes (prompted on first run):
        Group.ReadWrite.All
        DeviceManagementConfiguration.ReadWrite.All
        Directory.Read.All

    The signed-in account needs Intune Administrator (or equivalent
    deviceManagement/deviceConfigurations RW).
#>
[CmdletBinding()]
param(
    [string] $SharedCertProfileName  = 'Gell - SCEP Cert',
    [string] $TrustedRootConfigName  = 'Gell - Root Trusted Cert',
    [string] $TestDeviceGroupName    = 'cmtraceopen-testdevices',
    [string] $BuildMachineGroupName  = 'cmtraceopen-build-machines',
    [string] $TenantId
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ------------------------------------------------------------------------
# Module bootstrap
# ------------------------------------------------------------------------
$requiredModules = @(
    'Microsoft.Graph.Authentication',
    'Microsoft.Graph.Groups'
)
foreach ($m in $requiredModules) {
    if (-not (Get-Module -ListAvailable -Name $m)) {
        Write-Host "Installing module $m for the current user ..." -ForegroundColor Yellow
        Install-Module -Name $m -Scope CurrentUser -Force -AllowClobber | Out-Null
    }
    Import-Module $m -ErrorAction Stop
}

# ------------------------------------------------------------------------
# Sign in
# ------------------------------------------------------------------------
$scopes = @(
    'Group.ReadWrite.All',
    'DeviceManagementConfiguration.ReadWrite.All',
    'Directory.Read.All'
)
$connectArgs = @{ Scopes = $scopes; NoWelcome = $true }
if ($PSBoundParameters.ContainsKey('TenantId') -and $TenantId) {
    $connectArgs['TenantId'] = $TenantId
}
Connect-MgGraph @connectArgs | Out-Null

$ctx = Get-MgContext
if (-not $ctx) { throw 'Connect-MgGraph returned no context. Aborting.' }
Write-Host "Signed in to tenant: $($ctx.TenantId) as $($ctx.Account)" -ForegroundColor Cyan

# ------------------------------------------------------------------------
# Helpers
# ------------------------------------------------------------------------
function Get-OrCreateSecurityGroup {
    param([Parameter(Mandatory)][string] $DisplayName)
    $existing = @(Get-MgGroup -Filter "displayName eq '$DisplayName'" -All)
    if ($existing.Count -gt 1) { throw "Multiple groups named '$DisplayName'; resolve manually." }
    if ($existing.Count -eq 1) {
        Write-Host "Group '$DisplayName' exists (id=$($existing[0].Id))." -ForegroundColor DarkGray
        return $existing[0]
    }
    Write-Host "Creating security group '$DisplayName'" -ForegroundColor Green
    return New-MgGroup -DisplayName $DisplayName `
                       -MailEnabled:$false `
                       -MailNickname ($DisplayName -replace '[^a-zA-Z0-9]', '') `
                       -SecurityEnabled:$true `
                       -Description "cmtraceopen Intune provisioning — $DisplayName"
}

function Get-DeviceConfigByName {
    param([Parameter(Mandatory)][string] $DisplayName)
    $uri = "https://graph.microsoft.com/beta/deviceManagement/deviceConfigurations?`$filter=displayName eq '$([uri]::EscapeDataString($DisplayName))'"
    $resp = Invoke-MgGraphRequest -Method GET -Uri $uri
    if ($null -eq $resp -or -not $resp.ContainsKey('value') -or -not $resp['value']) {
        return @()
    }
    return @($resp['value'])
}

function Invoke-AssignConfigToGroup {
    param(
        [Parameter(Mandatory)][string] $ConfigId,
        [Parameter(Mandatory)][string[]] $GroupIds
    )
    $assignments = @()
    foreach ($gid in $GroupIds) {
        $assignments += @{
            target = @{
                '@odata.type' = '#microsoft.graph.groupAssignmentTarget'
                groupId       = $gid
            }
        }
    }
    $body = @{ assignments = $assignments } | ConvertTo-Json -Depth 10 -Compress
    Invoke-MgGraphRequest -Method POST `
        -Uri "https://graph.microsoft.com/beta/deviceManagement/deviceConfigurations/$ConfigId/assign" `
        -Body $body -ContentType 'application/json' | Out-Null
}

# ------------------------------------------------------------------------
# 1) Security groups
# ------------------------------------------------------------------------
$testGroup  = Get-OrCreateSecurityGroup -DisplayName $TestDeviceGroupName
$buildGroup = Get-OrCreateSecurityGroup -DisplayName $BuildMachineGroupName

# ------------------------------------------------------------------------
# 2) Look up the existing portal-created configs
# ------------------------------------------------------------------------
$rootMatches = @(Get-DeviceConfigByName -DisplayName $TrustedRootConfigName)
if ($rootMatches.Count -eq 0) {
    throw "Trusted-root device config '$TrustedRootConfigName' not found. Create it in Intune portal first (Tenant admin > Cloud PKI or Devices > Configuration > New > Trusted certificate)."
}
$rootConfigId = [string]$rootMatches[0]['id']
Write-Host "Found trusted-root config '$TrustedRootConfigName' (id=$rootConfigId)." -ForegroundColor DarkGray

$scepMatches = @(Get-DeviceConfigByName -DisplayName $SharedCertProfileName)
if ($scepMatches.Count -eq 0) {
    throw "SCEP cert profile '$SharedCertProfileName' not found. Create it in the Intune portal first (Devices > Configuration > New > SCEP certificate)."
}
$scepConfigId = [string]$scepMatches[0]['id']
Write-Host "Found SCEP cert profile '$SharedCertProfileName' (id=$scepConfigId)." -ForegroundColor DarkGray

# ------------------------------------------------------------------------
# 3) Assignments
# ------------------------------------------------------------------------
# The SCEP profile issues a cert with both Client Auth and Code Signing
# EKUs, so either group needs it. The trusted-root config is a hard
# prerequisite for the SCEP cert's chain to validate on the device.
Write-Host "Assigning '$TrustedRootConfigName' to both groups ..." -ForegroundColor Green
Invoke-AssignConfigToGroup -ConfigId $rootConfigId -GroupIds @($testGroup.Id, $buildGroup.Id)

Write-Host "Assigning '$SharedCertProfileName' to both groups ..." -ForegroundColor Green
Invoke-AssignConfigToGroup -ConfigId $scepConfigId -GroupIds @($testGroup.Id, $buildGroup.Id)

# ------------------------------------------------------------------------
# 4) Summary
# ------------------------------------------------------------------------
Write-Host ''
Write-Host '================ Provisioning complete ================' -ForegroundColor Cyan
Write-Host ("{0,-36} {1}" -f 'Tenant',                         $ctx.TenantId)
Write-Host ("{0,-36} {1}" -f 'Trusted-root config id',         $rootConfigId)
Write-Host ("{0,-36} {1}" -f 'SCEP cert profile id',           $scepConfigId)
Write-Host ("{0,-36} {1}" -f ($TestDeviceGroupName + ' id'),   $testGroup.Id)
Write-Host ("{0,-36} {1}" -f ($BuildMachineGroupName + ' id'), $buildGroup.Id)
Write-Host ''
Write-Host 'REMINDER: Confirm the SCEP profile has a URI SAN before relying on mTLS device identity.' -ForegroundColor Yellow
Write-Host "  Intune portal > Devices > Configuration > '$SharedCertProfileName' >" -ForegroundColor Yellow
Write-Host '  Subject alternative name > Attribute=URI, Value:' -ForegroundColor Yellow
Write-Host "    device://$($ctx.TenantId)/{{AAD_Device_ID}}" -ForegroundColor Yellow
Write-Host '  (Intune rejects {{TenantId}} as a variable — hardcode the tenant GUID; {{AAD_Device_ID}} is the only template.)' -ForegroundColor DarkGray
Write-Host ''
Write-Host 'Next steps:' -ForegroundColor Yellow
Write-Host "  1. Entra-join + Intune-enroll your Windows box."
Write-Host "  2. Add the device object to one or both Entra groups (via portal or):"
Write-Host "       `$dev = Get-MgDevice -Filter `"displayName eq 'cmtrace-runner-01'`""
Write-Host "       New-MgGroupMember -GroupId $($testGroup.Id)  -DirectoryObjectId `$dev.Id"
Write-Host "       New-MgGroupMember -GroupId $($buildGroup.Id) -DirectoryObjectId `$dev.Id"
Write-Host "  3. Wait 5-30 min (or force-sync from Intune) for the cert to land in LocalMachine\My."
Write-Host "  4. Verify on the device:"
Write-Host "       Get-ChildItem Cert:\LocalMachine\My | ?{ `$_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.2' }"
Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
