<#
.SYNOPSIS
    One-off reconfigure: overwrite %ProgramData%\CMTraceOpen\Agent\config.toml
    with the current-release defaults, then restart the service.

.DESCRIPTION
    The WiX component for config.toml is NeverOverwrite="yes" so operator
    edits survive MSI upgrades. Side effect: when the MSI ships a new
    default schedule (e.g. 0.1.2's switch from nightly to */15), existing
    installs keep the old file and ignore it.

    This script is run on-demand from NinjaOne (paste into Automation →
    Library → Windows Script, Run As System) when the default config
    needs to propagate without a full uninstall/reinstall cycle.

    What it does:
      1. Backs up the existing config.toml next to itself (.bak-YYYYmmddHHMMSS).
      2. Writes the new content (embedded below — edit once, roll out once).
      3. Restarts the CMTraceOpenAgent service and verifies it comes back up.

    Exit codes:
      0  — config rewritten, service healthy
      1  — config path missing (agent not installed)
      2  — service failed to restart
#>

[CmdletBinding()]
param(
    [string]$ConfigPath = 'C:\ProgramData\CMTraceOpen\Agent\config.toml'
)

$ErrorActionPreference = 'Stop'

$desired = @'
# CMTrace Open Agent — default configuration.
# Lives at %ProgramData%\CMTraceOpen\Agent\config.toml.

api_endpoint = "http://192.168.2.50:8080"
request_timeout_secs = 60
evidence_schedule = "*/15 * * * *"
queue_max_bundles = 50
log_level = "info"
device_id = ""
log_paths = [
  "C:\\Windows\\CCM\\Logs\\**\\*.log",
  "C:\\ProgramData\\Microsoft\\IntuneManagementExtension\\Logs\\**\\*.log",
  "C:\\Windows\\Logs\\DSRegCmd\\**\\*.log",
]

[mtls]
cert_store = "LocalMachine\\My"
issuer_cn_pattern = "issuing.gell.internal.cdw.lab"
required_eku = "1.3.6.1.5.5.7.3.2"
'@

function Stamp($m) { Write-Host ("[{0}] {1}" -f (Get-Date).ToString('HH:mm:ss'), $m) }

if (-not (Test-Path -LiteralPath (Split-Path $ConfigPath -Parent))) {
    Write-Error "Config directory not found: $(Split-Path $ConfigPath -Parent). Is the agent installed?"
    exit 1
}

if (Test-Path -LiteralPath $ConfigPath) {
    $bak = "$ConfigPath.bak-$((Get-Date).ToString('yyyyMMddHHmmss'))"
    Stamp "backing up existing config → $bak"
    Copy-Item -LiteralPath $ConfigPath -Destination $bak -Force
}

Stamp "writing new config to $ConfigPath"
# Use .NET IO so we get UTF8 without BOM — some TOML parsers choke on BOMs.
[System.IO.File]::WriteAllText($ConfigPath, $desired, [System.Text.UTF8Encoding]::new($false))

$svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
if (-not $svc) {
    Write-Error "CMTraceOpenAgent service not found — agent isn't installed."
    exit 1
}

function Stop-ServiceHard {
    param([string]$Name, [int]$TimeoutSec = 20)

    $s = Get-Service -Name $Name -ErrorAction Stop
    if ($s.Status -eq 'Stopped') { return }

    # Polite stop first. -NoWait returns immediately; we manage the wait
    # ourselves so a StopPending that never resolves doesn't hang the
    # whole NinjaOne script.
    try { Stop-Service -Name $Name -Force -NoWait -ErrorAction Stop } catch {
        Stamp "Stop-Service raised: $($_.Exception.Message) — falling through to process kill"
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $s.Refresh()
        if ($s.Status -eq 'Stopped') { return }
        Start-Sleep -Milliseconds 500
    }

    # Still running — find the PID via WMI/CIM and kill the process. sc.exe
    # queryex is the canonical way to get a service PID that also works when
    # the SCM itself is stuck.
    Stamp "service still $($s.Status) after ${TimeoutSec}s — force-killing process"
    $row = & sc.exe queryex $Name 2>$null | Select-String -Pattern 'PID\s*:\s*(\d+)'
    if ($row -and $row.Matches[0].Groups[1].Value) {
        $procPid = [int]$row.Matches[0].Groups[1].Value
        if ($procPid -gt 0) {
            Stamp "taskkill /F /PID $procPid"
            & taskkill.exe /F /PID $procPid | Out-Null
            Start-Sleep -Seconds 2
        }
    }

    $s.Refresh()
    if ($s.Status -ne 'Stopped') {
        throw "could not stop $Name (final status: $($s.Status))"
    }
}

Stamp "stopping CMTraceOpenAgent (hard)"
Stop-ServiceHard -Name 'CMTraceOpenAgent' -TimeoutSec 20

Stamp "starting CMTraceOpenAgent"
Start-Service -Name 'CMTraceOpenAgent'
Start-Sleep -Seconds 2

$svc.Refresh()
if ($svc.Status -ne 'Running') {
    Write-Error "Service is $($svc.Status) after start."
    exit 2
}
Stamp "service healthy. config refreshed."
exit 0
