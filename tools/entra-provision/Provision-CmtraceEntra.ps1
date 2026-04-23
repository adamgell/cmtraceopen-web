<#
.SYNOPSIS
    Provisions the two Entra app registrations (cmtraceopen-api +
    cmtraceopen-viewer) required for operator sign-in to the viewer.

.DESCRIPTION
    Interactive, idempotent Graph-driven provisioner. Mirrors the manual
    portal flow described in docs/provisioning/02-entra-app-registration.md:

        1. Signs in interactively to Microsoft Graph with the scopes needed
           to create app registrations and grant admin consent.
        2. Creates (or updates) the cmtraceopen-api app:
             - single tenant
             - identifierUri  api://<api-client-id>
             - delegated scope  CmtraceOpen.Query
             - app roles       CmtraceOpen.Operator, CmtraceOpen.Admin
           ...and its service principal.
        3. Creates (or updates) the cmtraceopen-viewer app:
             - single tenant, SPA platform
             - redirect URI  http://localhost:5173/  (override with -RedirectUri)
             - requested delegated permission on CmtraceOpen.Query
           ...and its service principal.
        4. Grants tenant-wide admin consent for the viewer on the api's
           CmtraceOpen.Query scope (oauth2PermissionGrant, AllPrincipals).
        5. Optionally assigns the signed-in operator to the CmtraceOpen.Admin
           app role so admin routes are reachable immediately.
        6. Writes the three VITE_ENTRA_* values to .env.local for the viewer
           (and prints the matching CMTRACE_ENTRA_* values for the api-server).

    Re-running the script against an existing tenant is safe: existing apps
    are detected by displayName and updated in place rather than duplicated.

.PARAMETER TenantId
    The Entra tenant to provision into. Optional; if omitted, the script
    uses whichever tenant Connect-MgGraph lands in and prints it for
    confirmation.

.PARAMETER ApiAppName
    Display name for the api app registration. Default: cmtraceopen-api.

.PARAMETER ViewerAppName
    Display name for the viewer SPA app registration.
    Default: cmtraceopen-viewer.

.PARAMETER RedirectUri
    SPA redirect URI to register on the viewer app. Pass additional URIs
    by supplying an array. Default: http://localhost:5173/.

.PARAMETER EnvLocalPath
    Path to write the VITE_ENTRA_* block. Default: <repo-root>/.env.local.
    Pass an empty string to skip writing.

.PARAMETER AssignCurrentUserAsAdmin
    If set, the signed-in user is granted the CmtraceOpen.Admin app role on
    cmtraceopen-api. Convenient for a first-run dev loop; skip in shared
    tenants where role assignment should go via a security group instead.

.PARAMETER SkipAdminConsent
    If set, the viewer's delegated CmtraceOpen.Query permission is left in
    the "pending admin consent" state. The signed-in operator will get a
    per-user consent prompt on first login instead.

.EXAMPLE
    pwsh ./tools/entra-provision/Provision-CmtraceEntra.ps1

    Interactive run, single tenant, localhost SPA redirect, writes
    .env.local, grants admin consent, and makes the signed-in user an
    admin on the api app.

.EXAMPLE
    pwsh ./tools/entra-provision/Provision-CmtraceEntra.ps1 `
        -TenantId 00000000-0000-0000-0000-000000000000 `
        -RedirectUri http://localhost:5173/, https://viewer.example.com/

    Multi-redirect-URI run against a specific tenant.

.NOTES
    Required Graph delegated scopes (you will be prompted to consent on
    first run):
        Application.ReadWrite.All       - create/update app registrations
        DelegatedPermissionGrant.ReadWrite.All
                                         - grant admin consent to the scope
        AppRoleAssignment.ReadWrite.All - assign the current user to Admin
        Directory.Read.All              - resolve the current user

    The signed-in account must hold Global Administrator or Application
    Administrator to create app registrations.
#>
[CmdletBinding()]
param(
    [string]   $TenantId,
    [string]   $ApiAppName    = 'cmtraceopen-api',
    [string]   $ViewerAppName = 'cmtraceopen-viewer',
    [string[]] $RedirectUri   = @('http://localhost:5173/'),
    [string]   $EnvLocalPath,
    [switch]   $AssignCurrentUserAsAdmin,
    [switch]   $SkipAdminConsent
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ------------------------------------------------------------------------
# Module bootstrap
# ------------------------------------------------------------------------
$requiredModules = @(
    'Microsoft.Graph.Authentication',
    'Microsoft.Graph.Applications',
    'Microsoft.Graph.Identity.SignIns',
    'Microsoft.Graph.Users'
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
    'Application.ReadWrite.All',
    'DelegatedPermissionGrant.ReadWrite.All',
    'AppRoleAssignment.ReadWrite.All',
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
function Get-OrCreateApplication {
    param(
        [Parameter(Mandatory)] [string] $DisplayName,
        [Parameter(Mandatory)] [hashtable] $BaseProps
    )
    # Force array so .Count is always defined (StrictMode-safe).
    $existing = @(Get-MgApplication -Filter "displayName eq '$DisplayName'" -All)
    if ($existing.Count -gt 1) {
        throw "Found multiple applications named '$DisplayName'. Resolve manually and re-run."
    }
    if ($existing.Count -eq 1) {
        Write-Host "App '$DisplayName' exists (appId=$($existing[0].AppId)); reusing." -ForegroundColor DarkGray
        return $existing[0]
    }
    Write-Host "Creating app '$DisplayName' ..." -ForegroundColor Green
    return New-MgApplication @BaseProps
}

function Get-OrCreateServicePrincipal {
    param([Parameter(Mandatory)] [string] $AppId)
    $sp = @(Get-MgServicePrincipal -Filter "appId eq '$AppId'" -All)
    if ($sp.Count -ge 1) { return $sp[0] }
    return New-MgServicePrincipal -AppId $AppId
}

# ------------------------------------------------------------------------
# 1) API app: cmtraceopen-api
# ------------------------------------------------------------------------
$apiApp = Get-OrCreateApplication -DisplayName $ApiAppName -BaseProps @{
    DisplayName      = $ApiAppName
    SignInAudience   = 'AzureADMyOrg'
}
$apiAppId    = $apiApp.AppId
$apiObjectId = $apiApp.Id

# Ensure identifierUri = api://<appId>
$desiredIdentifierUri = "api://$apiAppId"
$apiApp = Get-MgApplication -ApplicationId $apiObjectId
if (-not ($apiApp.IdentifierUris -contains $desiredIdentifierUri)) {
    Write-Host "Setting api app identifierUri to $desiredIdentifierUri" -ForegroundColor Green
    Update-MgApplication -ApplicationId $apiObjectId -IdentifierUris @($desiredIdentifierUri)
    $apiApp = Get-MgApplication -ApplicationId $apiObjectId
}

# Ensure the API app issues v2.0 tokens. By default new apps get null
# here, which makes Entra issue v1 tokens (iss = sts.windows.net/<tid>/).
# The api-server's expected_issuer() is the v2 form
# (login.microsoftonline.com/<tid>/v2.0) so v1 tokens fail with
# "Required issuer mismatch". Setting requestedAccessTokenVersion=2
# flips the issuer to match.
$currentVersion = $null
if ($apiApp.Api) { $currentVersion = $apiApp.Api.RequestedAccessTokenVersion }
if ($currentVersion -ne 2) {
    Write-Host "Setting API app requestedAccessTokenVersion=2 (was $currentVersion)" -ForegroundColor Green
    Update-MgApplication -ApplicationId $apiObjectId -Api @{ requestedAccessTokenVersion = 2 }
    $apiApp = Get-MgApplication -ApplicationId $apiObjectId
}

# --- Delegated scope: CmtraceOpen.Query ---------------------------------
$scopeValue = 'CmtraceOpen.Query'
$existingScopes = @()
if ($apiApp.Api -and $apiApp.Api.Oauth2PermissionScopes) {
    $existingScopes = @($apiApp.Api.Oauth2PermissionScopes)
}
$existingQueryScope = @($existingScopes | Where-Object { $_.Value -eq $scopeValue })
if ($existingQueryScope.Count -ge 1) {
    Write-Host "Delegated scope '$scopeValue' already present." -ForegroundColor DarkGray
    $queryScopeId = $existingQueryScope[0].Id
} else {
    Write-Host "Adding delegated scope '$scopeValue' to $ApiAppName" -ForegroundColor Green
    $queryScopeId = [guid]::NewGuid().ToString()
    $newScope = @{
        id                      = $queryScopeId
        adminConsentDescription = 'Allows the signed-in operator to run queries and retrieve CMTrace log data via the cmtraceopen api-server.'
        adminConsentDisplayName = 'Query cmtraceopen logs'
        userConsentDescription  = 'Let cmtraceopen run log queries on your behalf.'
        userConsentDisplayName  = 'Query logs via cmtraceopen'
        value                   = $scopeValue
        type                    = 'User'
        isEnabled               = $true
    }
    # Rebuild as hashtables so Graph accepts the payload regardless of the
    # shape returned by the read above.
    $rebuilt = @()
    foreach ($s in $existingScopes) {
        $rebuilt += @{
            id                      = $s.Id
            adminConsentDescription = $s.AdminConsentDescription
            adminConsentDisplayName = $s.AdminConsentDisplayName
            userConsentDescription  = $s.UserConsentDescription
            userConsentDisplayName  = $s.UserConsentDisplayName
            value                   = $s.Value
            type                    = $s.Type
            isEnabled               = $s.IsEnabled
        }
    }
    $rebuilt += $newScope
    Update-MgApplication -ApplicationId $apiObjectId -Api @{ oauth2PermissionScopes = $rebuilt }
    $apiApp = Get-MgApplication -ApplicationId $apiObjectId
}

# --- App roles: Operator + Admin ----------------------------------------
$desiredRoles = @(
    @{
        Value       = 'CmtraceOpen.Operator'
        DisplayName = 'CmtraceOpen Operator'
        Description = 'Read access to all device, session, and log-entry data via the cmtraceopen api-server.'
    },
    @{
        Value       = 'CmtraceOpen.Admin'
        DisplayName = 'CmtraceOpen Admin'
        Description = 'Operator privileges plus access to admin routes (device disable, future destructive admin actions).'
    }
)

$currentRoles = @()
if ($apiApp.AppRoles) { $currentRoles = @($apiApp.AppRoles) }
# Rebuild every role as a hashtable so the PATCH shape is consistent.
$rebuiltRoles = @()
foreach ($r in $currentRoles) {
    $rebuiltRoles += @{
        id                 = $r.Id
        allowedMemberTypes = @($r.AllowedMemberTypes)
        description        = $r.Description
        displayName        = $r.DisplayName
        isEnabled          = $r.IsEnabled
        value              = $r.Value
    }
}
$rolesChanged = $false
foreach ($r in $desiredRoles) {
    $match = @($currentRoles | Where-Object { $_.Value -eq $r.Value })
    if ($match.Count -eq 0) {
        Write-Host "Adding app role '$($r.Value)' to $ApiAppName" -ForegroundColor Green
        $rebuiltRoles += @{
            id                 = [guid]::NewGuid().ToString()
            allowedMemberTypes = @('User', 'Application')
            description        = $r.Description
            displayName        = $r.DisplayName
            isEnabled          = $true
            value              = $r.Value
        }
        $rolesChanged = $true
    }
}
if ($rolesChanged) {
    Update-MgApplication -ApplicationId $apiObjectId -AppRoles $rebuiltRoles
    $apiApp = Get-MgApplication -ApplicationId $apiObjectId
}

# SP for the api app (needed for oauth2PermissionGrant + role assignment)
$apiSp = Get-OrCreateServicePrincipal -AppId $apiAppId

# ------------------------------------------------------------------------
# 2) Viewer SPA: cmtraceopen-viewer
# ------------------------------------------------------------------------
$viewerApp = Get-OrCreateApplication -DisplayName $ViewerAppName -BaseProps @{
    DisplayName    = $ViewerAppName
    SignInAudience = 'AzureADMyOrg'
    Spa            = @{ RedirectUris = $RedirectUri }
}
$viewerAppId    = $viewerApp.AppId
$viewerObjectId = $viewerApp.Id

# Ensure SPA platform + redirect URIs are up to date (idempotent merge).
$viewerApp = Get-MgApplication -ApplicationId $viewerObjectId
$existingSpaUris = @()
if ($viewerApp.Spa -and $viewerApp.Spa.RedirectUris) {
    $existingSpaUris = @($viewerApp.Spa.RedirectUris)
}
$mergedSpaUris = @(@($existingSpaUris) + @($RedirectUri) | Select-Object -Unique)
$needSpaUpdate = $false
if ($existingSpaUris.Count -ne $mergedSpaUris.Count) { $needSpaUpdate = $true }
else {
    foreach ($u in $mergedSpaUris) { if ($existingSpaUris -notcontains $u) { $needSpaUpdate = $true; break } }
}
if ($needSpaUpdate) {
    Write-Host "Updating viewer SPA redirect URIs -> $($mergedSpaUris -join ', ')" -ForegroundColor Green
    Update-MgApplication -ApplicationId $viewerObjectId -Spa @{ redirectUris = $mergedSpaUris }
}

# --- Request delegated permission on api.CmtraceOpen.Query --------------
$existingRra = @()
if ($viewerApp.RequiredResourceAccess) { $existingRra = @($viewerApp.RequiredResourceAccess) }

$hasRra = $false
foreach ($rra in $existingRra) {
    if ($rra.ResourceAppId -ne $apiAppId) { continue }
    foreach ($ra in @($rra.ResourceAccess)) {
        if ($ra.Id -eq $queryScopeId -and $ra.Type -eq 'Scope') { $hasRra = $true; break }
    }
    if ($hasRra) { break }
}

if (-not $hasRra) {
    Write-Host "Adding delegated permission (viewer -> api.$scopeValue)" -ForegroundColor Green
    # Drop any prior entry for this api so we can replace cleanly.
    $others = @($existingRra | Where-Object { $_.ResourceAppId -ne $apiAppId })
    $rebuilt = @()
    foreach ($rra in $others) {
        $accessList = @()
        foreach ($ra in @($rra.ResourceAccess)) {
            $accessList += @{ id = $ra.Id; type = $ra.Type }
        }
        $rebuilt += @{ resourceAppId = $rra.ResourceAppId; resourceAccess = $accessList }
    }
    $rebuilt += @{
        resourceAppId  = $apiAppId
        resourceAccess = @(@{ id = $queryScopeId; type = 'Scope' })
    }
    Update-MgApplication -ApplicationId $viewerObjectId -RequiredResourceAccess $rebuilt
}

$viewerSp = Get-OrCreateServicePrincipal -AppId $viewerAppId

# ------------------------------------------------------------------------
# 3) Admin consent for the delegated scope (tenant-wide)
# ------------------------------------------------------------------------
if (-not $SkipAdminConsent) {
    $grants = @(Get-MgOauth2PermissionGrant -All | Where-Object {
            $_.ClientId -eq $viewerSp.Id -and $_.ResourceId -eq $apiSp.Id -and $_.ConsentType -eq 'AllPrincipals'
        })
    if ($grants.Count -ge 1) {
        $g = $grants[0]
        $currentScope = if ($g.Scope) { $g.Scope } else { '' }
        $scopeTokens = @($currentScope -split '\s+' | Where-Object { $_ })
        if ($scopeTokens -notcontains $scopeValue) {
            Write-Host "Extending existing admin-consent grant with $scopeValue" -ForegroundColor Green
            $merged = ((@($scopeTokens) + $scopeValue) | Select-Object -Unique) -join ' '
            Update-MgOauth2PermissionGrant -OAuth2PermissionGrantId $g.Id -Scope $merged
        } else {
            Write-Host "Admin consent already granted for $scopeValue." -ForegroundColor DarkGray
        }
    } else {
        Write-Host "Granting tenant-wide admin consent (viewer -> $scopeValue)" -ForegroundColor Green
        New-MgOauth2PermissionGrant -ClientId $viewerSp.Id -ConsentType 'AllPrincipals' `
            -ResourceId $apiSp.Id -Scope $scopeValue | Out-Null
    }
} else {
    Write-Host "Skipping admin consent (-SkipAdminConsent set)." -ForegroundColor Yellow
}

# ------------------------------------------------------------------------
# 4) Optional: assign the signed-in user to CmtraceOpen.Admin
# ------------------------------------------------------------------------
if ($AssignCurrentUserAsAdmin) {
    $me = $null
    try { $me = Get-MgUser -UserId $ctx.Account -ErrorAction Stop } catch { $me = $null }
    if (-not $me) {
        Write-Warning "Could not resolve signed-in user '$($ctx.Account)' for role assignment; skipping."
    } else {
        $adminRole = @($apiApp.AppRoles | Where-Object { $_.Value -eq 'CmtraceOpen.Admin' })
        if ($adminRole.Count -eq 0) { throw 'CmtraceOpen.Admin role not found on api app after provisioning.' }
        $adminRoleId = $adminRole[0].Id

        $assignments = @(Get-MgServicePrincipalAppRoleAssignedTo -ServicePrincipalId $apiSp.Id -All |
            Where-Object { $_.PrincipalId -eq $me.Id -and $_.AppRoleId -eq $adminRoleId })
        if ($assignments.Count -ge 1) {
            Write-Host "User $($ctx.Account) already has CmtraceOpen.Admin." -ForegroundColor DarkGray
        } else {
            Write-Host "Assigning CmtraceOpen.Admin to $($ctx.Account)" -ForegroundColor Green
            New-MgServicePrincipalAppRoleAssignedTo -ServicePrincipalId $apiSp.Id -BodyParameter @{
                principalId = $me.Id
                resourceId  = $apiSp.Id
                appRoleId   = $adminRoleId
            } | Out-Null
        }
    }
}

# ------------------------------------------------------------------------
# 5) Emit env block + .env.local
# ------------------------------------------------------------------------
$apiScopeFqdn = "api://$apiAppId/$scopeValue"
$apiAudience  = "api://$apiAppId"
$jwksUri      = "https://login.microsoftonline.com/$($ctx.TenantId)/discovery/v2.0/keys"

$viewerBlock = @"
# cmtraceopen-viewer (Entra)
VITE_ENTRA_TENANT_ID=$($ctx.TenantId)
VITE_ENTRA_CLIENT_ID=$viewerAppId
VITE_ENTRA_API_SCOPE=$apiScopeFqdn
"@

$apiBlock = @"
# cmtraceopen api-server (Entra JWT validation)
CMTRACE_ENTRA_TENANT_ID=$($ctx.TenantId)
CMTRACE_ENTRA_AUDIENCE=$apiAudience
CMTRACE_ENTRA_JWKS_URI=$jwksUri
"@

Write-Host ''
Write-Host '================ viewer .env.local ================' -ForegroundColor Cyan
Write-Host $viewerBlock
Write-Host ''
Write-Host '============ api-server environment ===============' -ForegroundColor Cyan
Write-Host $apiBlock
Write-Host ''

if (-not $PSBoundParameters.ContainsKey('EnvLocalPath')) {
    $repoRoot    = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $EnvLocalPath = Join-Path $repoRoot '.env.local'
}
if ($EnvLocalPath) {
    if (Test-Path $EnvLocalPath) {
        $backup = "$EnvLocalPath.bak"
        Copy-Item -Path $EnvLocalPath -Destination $backup -Force
        Write-Host "Existing $EnvLocalPath backed up to $backup" -ForegroundColor Yellow
    }
    Set-Content -Path $EnvLocalPath -Value $viewerBlock -Encoding utf8
    Write-Host "Wrote viewer env to $EnvLocalPath" -ForegroundColor Green
    Write-Host 'Restart the Vite dev server (pnpm dev) to pick up the new env.' -ForegroundColor Yellow
}

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
