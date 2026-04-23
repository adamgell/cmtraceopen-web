<#
.SYNOPSIS
    Invoke the (UI-saved) cmtraceopen-agent installer script on NinjaOne devices.

.DESCRIPTION
    Runs the NinjaOne PowerShell script (see install-cmtrace-agent.ps1) against a
    set of devices via the v2 API. NinjaOne's public API does NOT expose script
    library CRUD — scripts have to be created by hand in Administration →
    Library → Automation. Once saved, we can trigger them with:

        POST /v2/device/{deviceId}/script

    This driver:
      1. OAuths against ca.ninjarmm.com (reads creds from $HOME/.config/cmtrace/ninjaone.env
         unless -ClientId/-ClientSecret are passed explicitly).
      2. Looks up the target scriptId by -ScriptName (fuzzy name-contains, 1 match required).
      3. Enumerates the device fleet, filtered by -OrgId / -NodeClass / -HostnameLike.
      4. POSTs the invoke for each device, with optional concurrency throttle.

.PARAMETER ScriptName
    The name of the script inside NinjaOne (as it appears in the Automation library).
    Default: 'CMTraceOpen Agent — Install'.

.PARAMETER OrgId
    Only target devices in this organization id. Example: 2 ('Adam Gell Lab').

.PARAMETER NodeClass
    Only target devices whose nodeClass contains this string (e.g. 'WINDOWS_WORKSTATION').

.PARAMETER HostnameLike
    Only target devices whose systemName contains this substring (case-insensitive).

.PARAMETER DryRun
    Print the planned invocations and exit without hitting POST.

.PARAMETER EnvFile
    Path to a KEY=VALUE env file with NINJA_REGION/NINJA_CLIENT_ID/NINJA_CLIENT_SECRET.
    Default: $HOME/.config/cmtrace/ninjaone.env.

.EXAMPLE
    pwsh ./Invoke-CmtraceAgentInstall.ps1 -OrgId 2 -NodeClass WINDOWS -DryRun

.EXAMPLE
    pwsh ./Invoke-CmtraceAgentInstall.ps1 -OrgId 2 -HostnameLike gell
#>

[CmdletBinding()]
param(
    [string]$ScriptName = 'CMTraceOpen Agent — Install',
    [int]$OrgId,
    [string]$NodeClass,
    [string]$HostnameLike,
    [switch]$DryRun,
    [string]$EnvFile = "$HOME/.config/cmtrace/ninjaone.env",
    [string]$ClientId,
    [string]$ClientSecret,
    [string]$Region = 'ca.ninjarmm.com'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------- 1. Creds ----------

if ((-not $ClientId -or -not $ClientSecret) -and (Test-Path -LiteralPath $EnvFile)) {
    Get-Content -LiteralPath $EnvFile | ForEach-Object {
        $line = $_.Trim()
        if ($line -and -not $line.StartsWith('#') -and $line -match '^(?<k>[A-Z0-9_]+)=(?<v>.*)$') {
            switch ($Matches['k']) {
                'NINJA_CLIENT_ID'     { if (-not $ClientId)     { $ClientId = $Matches['v'] } }
                'NINJA_CLIENT_SECRET' { if (-not $ClientSecret) { $ClientSecret = $Matches['v'] } }
                'NINJA_REGION'        { if ($Region -eq 'ca.ninjarmm.com') { $Region = $Matches['v'] } }
            }
        }
    }
}
if (-not $ClientId -or -not $ClientSecret) {
    throw "Missing NinjaOne creds. Set them in $EnvFile or pass -ClientId / -ClientSecret."
}

$base = "https://$Region"

# ---------- 2. Token ----------

$body = @{
    grant_type    = 'client_credentials'
    client_id     = $ClientId
    client_secret = $ClientSecret
    scope         = 'monitoring management'
}
$tok = Invoke-RestMethod -Method Post `
    -Uri "$base/ws/oauth/token" `
    -ContentType 'application/x-www-form-urlencoded' `
    -Body $body
$headers = @{ Authorization = "Bearer $($tok.access_token)" }

# ---------- 3. Script lookup ----------
# NOTE: /v2/automation/scripts returns the script library. This is read-only via
# the public API. We match on name (contains, case-insensitive) and require
# exactly one hit so a rename-to-duplicate doesn't silently run the wrong script.

$scripts = Invoke-RestMethod -Method Get -Uri "$base/v2/automation/scripts" -Headers $headers
$match = @($scripts | Where-Object { $_.name -like "*$ScriptName*" })
if ($match.Count -eq 0) {
    throw "No NinjaOne script found matching name '*$ScriptName*'. Create it in Administration → Library → Automation first."
}
if ($match.Count -gt 1) {
    $names = ($match | ForEach-Object { "[$($_.id)] $($_.name)" }) -join ', '
    throw "Multiple scripts match '*$ScriptName*': $names. Make the -ScriptName more specific."
}
$scriptId = $match[0].id
Write-Host "Script: [$scriptId] $($match[0].name)"

# ---------- 4. Device enumeration ----------
# PS 7 quirk: Invoke-RestMethod on a JSON array returns a collection that
# `@(...)` wrapping can double-wrap into a single-element outer array (whose
# one member is the 18-device inner array — you then see `System.Object[]`
# where you expected scalars). ForEach-Object with an explicit `$_` inside an
# array-initializer subexpression sidesteps that and flattens cleanly.

$devicesRaw = Invoke-RestMethod -Method Get -Uri "$base/v2/devices" -Headers $headers
$filtered = @($devicesRaw | ForEach-Object { $_ })
if ($OrgId) {
    $filtered = @($filtered | Where-Object { $_.organizationId -eq $OrgId })
}
if ($NodeClass) {
    $filtered = @($filtered | Where-Object { $_.nodeClass -like "*$NodeClass*" })
}
if ($HostnameLike) {
    $filtered = @($filtered | Where-Object { $_.systemName -and $_.systemName.ToLower().Contains($HostnameLike.ToLower()) })
}

Write-Host "Targets: $($filtered.Count) device(s)"
foreach ($d in $filtered) {
    Write-Host ("  {0,6}  {1,-24} org={2,-3} class={3}" -f $d.id, ($d.systemName ?? '(no name)'), $d.organizationId, $d.nodeClass)
}

if ($DryRun) {
    Write-Host ""
    Write-Host "DryRun — not invoking. Re-run without -DryRun to execute." -ForegroundColor Yellow
    return
}

# ---------- 5. Invoke ----------
$results = foreach ($d in $filtered) {
    $url = "$base/v2/device/$($d.id)/script"
    $payload = @{ id = $scriptId; runAs = 'system' } | ConvertTo-Json
    try {
        $r = Invoke-RestMethod -Method Post -Uri $url -Headers $headers `
            -ContentType 'application/json' -Body $payload
        [pscustomobject]@{ deviceId = $d.id; host = $d.systemName; ok = $true; result = $r }
    } catch {
        [pscustomobject]@{ deviceId = $d.id; host = $d.systemName; ok = $false; error = $_.Exception.Message }
    }
}

$ok   = @($results | Where-Object { $_.ok }).Count
$fail = @($results | Where-Object { -not $_.ok }).Count
Write-Host ""
Write-Host "Invoked: $ok ok, $fail failed" -ForegroundColor ($(if ($fail) { 'Yellow' } else { 'Green' }))
$results | Where-Object { -not $_.ok } | ForEach-Object {
    Write-Host "  FAILED $($_.deviceId) $($_.host): $($_.error)" -ForegroundColor Red
}
$results
