<#
.SYNOPSIS
    Pre-install stopper for the CMTrace Open Agent service.

.DESCRIPTION
    Wired in as the **Pre-Script** of a NinjaOne installer policy (or the
    Intune Win32 app's pre-install command). Hard-stops the
    `CMTraceOpenAgent` service so the subsequent `msiexec /i ...` can
    replace `cmtraceopen-agent.exe` without hitting a Windows file lock.

    Failure mode this exists to prevent: MSI install completes, registry
    bumps to the new version (Add/Remove Programs shows e.g. "0.2.0"),
    but the .exe on disk wasn't actually swapped because the running
    service held a lock on it. The service keeps running the old binary
    until something forces a true restart with the new file present.

    Behavior:
      1. If the service isn't installed at all (clean-fleet case) → exit 0.
      2. If already stopped → exit 0, no-op.
      3. Stop-Service -Force + bounded wait.
      4. If still in StopPending past the deadline → taskkill the process
         by PID via `sc.exe queryex`.
      5. Final state confirmed Stopped → exit 0; otherwise exit 2.

    Run As: System (NinjaOne Pre-Script default).

    Exit codes:
      0 — service stopped (or not installed, or already stopped)
      2 — could not stop within the deadline (rare; investigate manually)

.NOTES
    Pair with `reconfigure-cmtrace-agent.ps1` as the post-install script
    so the rollout sequence is:
      1. Pre:  prestop-cmtrace-agent.ps1   ← this file
      2. MSI:  msiexec /i CMTraceOpenAgent-<version>.msi /qn /norestart
      3. Post: reconfigure-cmtrace-agent.ps1   (writes config, restarts)
#>

[CmdletBinding()]
param(
    [string]$ServiceName  = 'CMTraceOpenAgent',
    [int]   $TimeoutSec   = 20
)

$ErrorActionPreference = 'Continue'   # we manage failures explicitly

function Stamp($m) { Write-Host ("[{0}] {1}" -f (Get-Date).ToString('HH:mm:ss'), $m) }

# Case 1: service not installed (fresh-fleet install).
$svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if (-not $svc) {
    Stamp "$ServiceName not installed — nothing to stop. Letting MSI proceed."
    exit 0
}

# Case 2: already stopped.
if ($svc.Status -eq 'Stopped') {
    Stamp "$ServiceName already stopped."
    exit 0
}

Stamp "stopping $ServiceName (current status: $($svc.Status))"

# Polite stop first. -NoWait so a StopPending that never resolves doesn't
# hang the whole NinjaOne pre-script.
try {
    Stop-Service -Name $ServiceName -Force -NoWait -ErrorAction Stop
} catch {
    Stamp "Stop-Service raised: $($_.Exception.Message) — falling through to process kill"
}

$deadline = (Get-Date).AddSeconds($TimeoutSec)
while ((Get-Date) -lt $deadline) {
    $svc.Refresh()
    if ($svc.Status -eq 'Stopped') {
        Stamp "$ServiceName stopped cleanly."
        exit 0
    }
    Start-Sleep -Milliseconds 500
}

# Still running after the deadline — find the PID via sc.exe queryex
# (works even when the SCM itself is the thing that's stuck) and kill
# the process directly.
Stamp "$ServiceName still $($svc.Status) after ${TimeoutSec}s — force-killing process"
$row = & sc.exe queryex $ServiceName 2>$null | Select-String -Pattern 'PID\s*:\s*(\d+)'
if ($row -and $row.Matches[0].Groups[1].Value) {
    $procPid = [int]$row.Matches[0].Groups[1].Value
    if ($procPid -gt 0) {
        Stamp "taskkill /F /PID $procPid"
        & taskkill.exe /F /PID $procPid | Out-Null
        Start-Sleep -Seconds 2
    }
}

# Belt-and-suspenders: kill any orphan cmtraceopen-agent.exe processes
# that aren't owned by the service controller (rare, but possible if a
# previous install left a stale child).
Get-Process -Name 'cmtraceopen-agent' -ErrorAction SilentlyContinue | ForEach-Object {
    Stamp "killing orphan cmtraceopen-agent.exe PID $($_.Id)"
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}

# Final confirmation.
Start-Sleep -Seconds 1
$svc.Refresh()
if ($svc.Status -eq 'Stopped') {
    Stamp "$ServiceName stopped after force-kill."
    exit 0
}

Stamp "ERROR: $ServiceName final status $($svc.Status) — could not stop."
exit 2
