<#
.SYNOPSIS
    Swap the CMTrace Open agent endpoint to pilot.cmtrace.net and restart.

.DESCRIPTION
    NinjaOne script (Automation > Library > Windows Script, Run As System).
    Rewrites api_endpoint in config.toml from the current value to
    https://pilot.cmtrace.net, backs up the old config, restarts the service.

    Exit codes:
      0 — endpoint swapped, service healthy
      1 — agent not installed
      2 — service failed to restart
#>

$ErrorActionPreference = 'Stop'
$ConfigPath = 'C:\ProgramData\CMTraceOpen\Agent\config.toml'
$NewEndpoint = 'https://pilot.cmtrace.net'

function Stamp($m) { Write-Host ("[{0}] {1}" -f (Get-Date).ToString('HH:mm:ss'), $m) }

if (-not (Test-Path -LiteralPath $ConfigPath)) {
    Write-Error "config.toml not found at $ConfigPath — agent not installed."
    exit 1
}

$bak = "$ConfigPath.bak-$((Get-Date).ToString('yyyyMMddHHmmss'))"
Stamp "backing up → $bak"
Copy-Item -LiteralPath $ConfigPath -Destination $bak -Force

$content = [System.IO.File]::ReadAllText($ConfigPath)
$original = $content

if ($content -match 'api_endpoint\s*=\s*"[^"]*"') {
    $content = $content -replace 'api_endpoint\s*=\s*"[^"]*"', "api_endpoint = `"$NewEndpoint`""
} else {
    $content = "api_endpoint = `"$NewEndpoint`"`n" + $content
}

if ($content -eq $original) {
    Stamp "endpoint already set to $NewEndpoint — no change needed"
} else {
    Stamp "writing new endpoint: $NewEndpoint"
    [System.IO.File]::WriteAllText($ConfigPath, $content, [System.Text.UTF8Encoding]::new($false))
}

$svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
if (-not $svc) {
    Write-Error "CMTraceOpenAgent service not found."
    exit 1
}

Stamp "restarting CMTraceOpenAgent"
Restart-Service -Name 'CMTraceOpenAgent' -Force
Start-Sleep -Seconds 3

$svc.Refresh()
if ($svc.Status -ne 'Running') {
    Write-Error "Service is $($svc.Status) after restart."
    exit 2
}

Stamp "done — endpoint swapped to $NewEndpoint, service running"
exit 0
