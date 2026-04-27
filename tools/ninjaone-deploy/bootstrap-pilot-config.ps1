<#
.SYNOPSIS
    Create or overwrite config.toml pointing at pilot.cmtrace.net, then
    restart the agent. Works whether or not config.toml already exists.

    NinjaOne: Automation > Library > Windows Script, Run As System.

    Exit codes: 0 = success, 1 = agent exe not found, 2 = service failed
#>

$ErrorActionPreference = 'Stop'
$ConfigDir  = 'C:\ProgramData\CMTraceOpen\Agent'
$ConfigPath = Join-Path $ConfigDir 'config.toml'

$config = @'
api_endpoint = "https://pilot.cmtrace.net"
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

# Verify agent exe exists
$exePath = 'C:\Program Files (x86)\CMTraceOpen\Agent\cmtraceopen-agent.exe'
if (-not (Test-Path $exePath)) {
    $exePath = 'C:\Program Files\CMTraceOpen\Agent\cmtraceopen-agent.exe'
}
if (-not (Test-Path $exePath)) {
    Write-Error "Agent exe not found in Program Files — MSI not installed."
    exit 1
}

# Create ProgramData directory if missing
if (-not (Test-Path $ConfigDir)) {
    Stamp "creating $ConfigDir"
    New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
}

# Backup existing config if present
if (Test-Path $ConfigPath) {
    $bak = "$ConfigPath.bak-$((Get-Date).ToString('yyyyMMddHHmmss'))"
    Stamp "backing up existing config → $bak"
    Copy-Item -LiteralPath $ConfigPath -Destination $bak -Force
}

Stamp "writing config to $ConfigPath"
[System.IO.File]::WriteAllText($ConfigPath, $config, [System.Text.UTF8Encoding]::new($false))

# Restart the service
$svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
if ($svc) {
    Stamp "restarting CMTraceOpenAgent"
    Restart-Service -Name 'CMTraceOpenAgent' -Force
    Start-Sleep -Seconds 3
    $svc.Refresh()
    if ($svc.Status -ne 'Running') {
        Write-Error "Service is $($svc.Status) after restart."
        exit 2
    }
    Stamp "service running"
} else {
    Stamp "CMTraceOpenAgent service not found — config written but service needs manual start"
}

Stamp "done — endpoint: https://pilot.cmtrace.net"
exit 0
