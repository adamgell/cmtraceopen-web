<#
.SYNOPSIS
    Restart the CMTrace Open agent service. NinjaOne: Run As System.

    Exit codes: 0 = running, 1 = not installed, 2 = failed to start
#>

$svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
if (-not $svc) {
    Write-Host "CMTraceOpenAgent service not found."
    exit 1
}

Write-Host "Status: $($svc.Status)"
Restart-Service -Name 'CMTraceOpenAgent' -Force
Start-Sleep -Seconds 3

$svc.Refresh()
Write-Host "After restart: $($svc.Status)"
if ($svc.Status -ne 'Running') { exit 2 }
exit 0
