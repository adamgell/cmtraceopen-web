<#
.SYNOPSIS
    NinjaOne device-side installer for cmtraceopen-agent.

.DESCRIPTION
    This is the PowerShell payload that lives inside NinjaOne (Administration →
    Library → Automation → New → Windows Script). NinjaOne runs it on each target
    device with SYSTEM privileges as part of a script policy or ad-hoc action.

    What it does on the endpoint:
      1. If the agent service is already at or above $TargetVersion, exit 0.
      2. Download the signed MSI from a GitHub Release asset URL.
      3. Verify SHA256 (optional but recommended; pin a value once and forget).
      4. msiexec /i /qn /norestart — silent install, logs to %WINDIR%\Temp.
      5. Verify the service is installed + running; emit NinjaOne custom-field
         values for version + last-install timestamp (when the device ID exposes
         the custom-fields module to scripts).

    Idempotent: re-running against an up-to-date device is a no-op.

    Exit codes (NinjaOne reads these — non-zero = failed deployment):
      0  — already up to date, or fresh install succeeded
      1  — download/verification failed
      2  — msiexec failed (see install log path printed to stdout)
      3  — post-install service check failed
      10 — unsupported OS / prereq missing

.PARAMETER MsiUrl
    HTTPS URL to the signed MSI. Default points at the GitHub Release asset
    produced by `.github/workflows/agent-msi.yml` on tag `agent-v$TargetVersion`.

.PARAMETER TargetVersion
    Semver the endpoint should converge to. Used for the skip-if-current check
    and for composing the default MsiUrl.

.PARAMETER ExpectedSha256
    Hex SHA256 of the MSI. If provided, install aborts on mismatch. Recommended:
    pin this when you cut the release, so a compromised release asset can't
    replace the binary silently. Leave empty to skip the check (prints a warning).

.PARAMETER Reinstall
    Force msiexec /i even when the installed version matches TargetVersion.
    Equivalent to a repair.

.NOTES
    NinjaOne script setup:
      - Script Type     : PowerShell
      - Language        : PowerShell
      - Architecture    : Any   (we don't care — this runs 64-bit on Win10+)
      - Run As          : System
      - Timeout         : 300s or higher (download + msiexec)
      - Parameters      : leave empty to use defaults, or override per-policy.

    Troubleshooting on the endpoint:
      Get-Service CMTraceOpenAgent
      Get-ItemProperty HKLM:\SOFTWARE\CMTraceOpen\Agent
      Get-Content "$env:WINDIR\Temp\CMTraceOpenAgent-install.log" -Tail 40
#>

[CmdletBinding()]
param(
    [string]$TargetVersion = '0.1.3',

    [string]$MsiUrl,

    # Pinned to the 0.1.3 release asset hash computed on the build runner.
    # Re-pin whenever $TargetVersion changes — see the README for the
    # post-release workflow.
    [string]$ExpectedSha256 = '858ee038f7dc132c7087fe1be27999b6986377e5cb2e2b709dd79ba68def8531',

    [switch]$Reinstall
)

$ErrorActionPreference = 'Stop'
# NOTE: deliberately NOT Set-StrictMode. Some HKLM Uninstall subkeys ship
# without DisplayName/DisplayVersion properties; iterating them with
# `Where-Object { $_.DisplayName -like ... }` throws PropertyNotFoundStrict
# under StrictMode Latest before we ever reach msiexec. Strict on a one-shot
# SYSTEM deploy script is more footgun than safety net.

# ---------------------------------------------------------------------------
# Helpers

function Write-Stamp([string]$msg) {
    $t = (Get-Date).ToString('HH:mm:ss')
    Write-Host "[$t] $msg"
}

function Get-InstalledVersion {
    # cmtraceopen-agent writes DisplayVersion under HKLM Uninstall when the MSI
    # lays itself down. Preferring a single, well-known ProductName substring
    # over iterating both 32/64-bit Uninstall keys.
    $paths = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    foreach ($p in $paths) {
        $hit = Get-ItemProperty -Path $p -ErrorAction SilentlyContinue |
        Where-Object {
            $_.PSObject.Properties['DisplayName'] -and
            $_.DisplayName -like 'CMTrace Open Agent*'
        } |
        Select-Object -First 1
        if ($hit -and $hit.PSObject.Properties['DisplayVersion']) {
            return $hit.DisplayVersion
        }
    }
    return $null
}

function Compare-SemVer([string]$a, [string]$b) {
    # Returns -1 / 0 / 1. Treats missing values as lowest.
    if (-not $a) { return -1 }
    if (-not $b) { return 1 }
    try {
        return ([version]$a).CompareTo([version]$b)
    }
    catch {
        # Fall back to string compare for non-numeric pre-release suffixes.
        return [string]::Compare($a, $b, $true)
    }
}

function Test-ServiceHealthy {
    $svc = Get-Service -Name 'CMTraceOpenAgent' -ErrorAction SilentlyContinue
    if (-not $svc) { return $false }
    if ($svc.Status -ne 'Running') {
        # Give it a beat — service might still be spinning up post-install.
        Start-Sleep -Seconds 3
        $svc.Refresh()
    }
    return $svc.Status -eq 'Running'
}

# ---------------------------------------------------------------------------
# Main

if ([System.Environment]::OSVersion.Version.Major -lt 10) {
    Write-Error "CMTraceOpen Agent requires Windows 10 or later. Detected: $([System.Environment]::OSVersion.VersionString)"
    exit 10
}

if (-not $MsiUrl) {
    $MsiUrl = "https://github.com/adamgell/cmtraceopen-web/releases/download/agent-v$TargetVersion/CMTraceOpenAgent-$TargetVersion.msi"
}

Write-Stamp "cmtraceopen-agent deploy — target $TargetVersion"
Write-Stamp "host: $env:COMPUTERNAME"
Write-Stamp "msi : $MsiUrl"

# ---- 1. Skip if already current ----
$installed = Get-InstalledVersion
if ($installed) {
    Write-Stamp "installed version: $installed"
    $cmp = Compare-SemVer $installed $TargetVersion
    if ($cmp -ge 0 -and -not $Reinstall) {
        Write-Stamp "already at target version — nothing to do."
        if (Test-ServiceHealthy) {
            Write-Stamp "service healthy. exit 0."
            exit 0
        }
        Write-Stamp "service is not running — forcing reinstall."
    }
}
else {
    Write-Stamp "installed version: none"
}

# ---- 2. Download ----
$tmp = Join-Path $env:TEMP ("cmtraceopen-agent-{0}.msi" -f ([guid]::NewGuid().ToString('N')))
Write-Stamp "downloading to $tmp"
try {
    # TLS 1.2 is default on Win10 1903+, but pin it — there are still some
    # older servers out there that try to renegotiate down.
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $MsiUrl -OutFile $tmp -UseBasicParsing -ErrorAction Stop
}
catch {
    Write-Error "download failed: $($_.Exception.Message)"
    exit 1
}

$sizeKb = [math]::Round((Get-Item $tmp).Length / 1KB, 0)
Write-Stamp "downloaded $sizeKb KB"

# ---- 3. Verify ----
if ($ExpectedSha256) {
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash.ToUpperInvariant()
    $want = $ExpectedSha256.ToUpperInvariant()
    if ($actual -ne $want) {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
        Write-Error "sha256 mismatch. expected=$want actual=$actual"
        exit 1
    }
    Write-Stamp "sha256 verified: $actual"
}
else {
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash.ToUpperInvariant()
    Write-Stamp "sha256 (unpinned): $actual"
    Write-Warning "ExpectedSha256 was not provided. Pin this hash in the NinjaOne script parameters on a trusted run."
}

# ---- 4. Install ----
$installLog = Join-Path $env:WINDIR "Temp\CMTraceOpenAgent-install.log"
$msiArgs = @(
    '/i', "`"$tmp`"",
    '/qn',
    '/norestart',
    "/l*v", "`"$installLog`""
)
Write-Stamp "msiexec $($msiArgs -join ' ')"
$proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList $msiArgs -Wait -PassThru
$code = $proc.ExitCode
Write-Stamp "msiexec exit code: $code"

# Clean up the downloaded MSI regardless — we have the install log.
Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue

# MSI exit codes:
#   0    = success
#   3010 = success, reboot required
#   1638 = another version already installed at this major/minor (shouldn't hit)
#   1603 = fatal install error — check the verbose log
if ($code -ne 0 -and $code -ne 3010) {
    # Pull the decisive lines out of the verbose MSI log and surface them in
    # NinjaOne's activity output. Without this an operator sees "exit 1603"
    # and has to RMM into the box to read Temp\CMTraceOpenAgent-install.log.
    Write-Host ""
    Write-Host "---- install log (tail, $installLog) ----"
    if (Test-Path -LiteralPath $installLog) {
        # Only the lines that actually diagnose a failure. `Return value 3`
        # marks the rolled-back custom action; `Product: ... -- ...` carries
        # the localized error; "Note: 1:" lines carry the Windows Installer
        # diagnostic codes. Dump the last 60 lines as a safety net in case
        # none of those patterns match this particular failure mode.
        $log = Get-Content -LiteralPath $installLog -ErrorAction SilentlyContinue
        if ($log) {
            $interesting = $log | Select-String -Pattern 'Return value 3|Error \d+\.|Product:|Note: 1:|CustomAction.*returned' -SimpleMatch:$false
            if ($interesting) {
                $interesting | Select-Object -Last 20 | ForEach-Object { Write-Host "  $($_.Line)" }
                Write-Host "  ---- last 60 lines ----"
            }
            $log | Select-Object -Last 60 | ForEach-Object { Write-Host "  $_" }
        } else {
            Write-Host "  (log file exists but is empty)"
        }
    } else {
        Write-Host "  (install log not found — msiexec may have crashed before opening it)"
    }
    Write-Error "msiexec failed with code $code. see $installLog"
    exit 2
}

# ---- 5. Post-install check ----
$postVersion = Get-InstalledVersion
Write-Stamp "post-install version: $postVersion"

if (-not (Test-ServiceHealthy)) {
    Write-Error "CMTraceOpenAgent service is not running after install. see $installLog"
    exit 3
}
Write-Stamp "service healthy."

# NinjaOne custom fields (optional) — populated when the target device has the
# field configured in the UI. Failing here must not fail the whole script.
try {
    if (Get-Command -Name 'Ninja-Property-Set' -ErrorAction SilentlyContinue) {
        Ninja-Property-Set cmtraceAgentVersion   $postVersion
        Ninja-Property-Set cmtraceAgentLastInstall ((Get-Date).ToString('o'))
        Write-Stamp "ninja custom fields updated."
    }
}
catch {
    Write-Warning "ninja custom-field update skipped: $($_.Exception.Message)"
}

Write-Stamp "done. exit 0."
exit 0
