<#
.SYNOPSIS
    Bootstraps the non-cmtrace prerequisites for the self-hosted runner
    on a fresh Windows box: Git (machine scope, on Machine PATH) and the
    Visual Studio 2022 Build Tools with the C++ workload (needed for the
    Rust MSVC target used by agent-msi.yml).

.DESCRIPTION
    Run this ONCE, elevated, on a freshly-enrolled Windows box BEFORE
    Install-CmtraceRunner.ps1. Idempotent -- re-running after a partial
    success picks up where it left off.

    Uses direct installer downloads (no winget) so it works on Windows
    10, 11, and Server SKUs regardless of whether App Installer / winget
    is available to the current admin session.

    Steps:
        1. Elevation + TLS 1.2 sanity.
        2. Download + run Git for Windows installer with /ALLUSERS and
           path-option adds Program Files\Git\cmd + \bin.
        3. Ensure those two directories are on Machine PATH so the
           runner's service account inherits them.
        4. Download + run Visual Studio 2022 Build Tools bootstrapper
           (vs_BuildTools.exe) with the VCTools workload + Windows 11
           SDK. Puts link.exe on the box and wires up MSVC for Rust's
           MSVC target.
        5. Verifies git.exe, bash.exe, signtool.exe, and MSVC link.exe
           are all locatable.
        6. Restarts the runner service if it exists, so the runner
           process inherits the freshly-updated PATH.

.PARAMETER SkipGit
    Skip step 2/3. Useful if Git is already installed via another method.

.PARAMETER SkipBuildTools
    Skip step 4. Useful if VS 2022 or later is already installed with
    the C++ workload.

.NOTES
    Runs on Windows PowerShell 5.1 and PowerShell 7+. ASCII-only.
    Requires outbound https to github.com, aka.ms, visualstudio.com,
    and download.visualstudio.microsoft.com.
#>
#Requires -Version 5.1
[CmdletBinding()]
param(
    [switch] $SkipGit,
    [switch] $SkipBuildTools
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

# ------------------------------------------------------------------------
# Elevation
# ------------------------------------------------------------------------
$currentPrincipal = New-Object System.Security.Principal.WindowsPrincipal(
    [System.Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $currentPrincipal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this in an elevated PowerShell (Run as Administrator).'
}

# ------------------------------------------------------------------------
# Helpers
# ------------------------------------------------------------------------
function Add-ToMachinePath {
    param([Parameter(Mandatory)][string] $Dir)
    if (-not (Test-Path -LiteralPath $Dir)) {
        Write-Warning "Path '$Dir' does not exist; skipping Machine-PATH add."
        return
    }
    $mp = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $segments = @($mp -split ';')
    if ($segments -contains $Dir) {
        Write-Host "  Machine PATH already contains $Dir" -ForegroundColor DarkGray
        return
    }
    Write-Host "  Adding $Dir to Machine PATH" -ForegroundColor Green
    [Environment]::SetEnvironmentVariable('Path', "$mp;$Dir", 'Machine')
    # Update the current process PATH too so subsequent checks in this
    # script see the change without a restart.
    $env:PATH = "$env:PATH;$Dir"
}

function Invoke-Download {
    param(
        [Parameter(Mandatory)][string] $Uri,
        [Parameter(Mandatory)][string] $OutFile
    )
    Write-Host "  Downloading $Uri" -ForegroundColor DarkGray
    $savedProg = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'   # ~100x faster on PS 5.1
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing
    } finally {
        $ProgressPreference = $savedProg
    }
}

# ------------------------------------------------------------------------
# 1) Git for Windows (direct installer download, all-users scope)
# ------------------------------------------------------------------------
if ($SkipGit) {
    Write-Host 'Skipping Git install (-SkipGit).' -ForegroundColor DarkGray
} elseif (Test-Path -LiteralPath 'C:\Program Files\Git\cmd\git.exe') {
    Write-Host 'Git already installed at C:\Program Files\Git.' -ForegroundColor DarkGray
    Add-ToMachinePath -Dir 'C:\Program Files\Git\cmd'
    Add-ToMachinePath -Dir 'C:\Program Files\Git\bin'
} else {
    Write-Host 'Installing Git for Windows (all-users) ...' -ForegroundColor Cyan
    $rel = Invoke-RestMethod 'https://api.github.com/repos/git-for-windows/git/releases/latest' `
             -Headers @{ 'User-Agent' = 'cmtraceopen-prereqs' }
    $asset = $rel.assets |
        Where-Object { $_.name -match '^Git-.*-64-bit\.exe$' } |
        Select-Object -First 1
    if (-not $asset) { throw 'Could not find a 64-bit Git installer in the latest release.' }

    $gitExe = Join-Path $env:TEMP $asset.name
    Invoke-Download -Uri $asset.browser_download_url -OutFile $gitExe

    # /VERYSILENT + /SUPPRESSMSGBOXES = no UI. /NORESTART never reboots.
    # /ALLUSERS = all-users scope (lands under C:\Program Files\Git).
    # /COMPONENTS= keeps gitconfig + Git Bash; skips shell integrations
    # we don't need on a headless runner.
    # /o:PathOption=Cmd ensures the installer adds Program Files\Git\cmd
    # to Machine PATH itself, though we also add both dirs below as a
    # belt-and-suspenders for partial/failed PATH edits.
    Write-Host '  Running installer (silent) ...' -ForegroundColor DarkGray
    $proc = Start-Process -FilePath $gitExe `
                -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', `
                              '/NOCANCEL', '/SP-', '/ALLUSERS', `
                              '/o:PathOption=Cmd' `
                -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        throw "Git installer exited with code $($proc.ExitCode)."
    }
    Remove-Item -LiteralPath $gitExe -Force -ErrorAction SilentlyContinue
    Add-ToMachinePath -Dir 'C:\Program Files\Git\cmd'
    Add-ToMachinePath -Dir 'C:\Program Files\Git\bin'
}

# ------------------------------------------------------------------------
# 2) Visual Studio 2022 Build Tools (C++ workload + Windows 11 SDK)
#    Bootstrapper direct from aka.ms -- no winget required.
# ------------------------------------------------------------------------
if ($SkipBuildTools) {
    Write-Host 'Skipping VS Build Tools install (-SkipBuildTools).' -ForegroundColor DarkGray
} else {
    Write-Host 'Installing Visual Studio 2022 Build Tools (VCTools + Win11 SDK) ...' -ForegroundColor Cyan
    Write-Host '  This download is multi-GB and may take 5-10 minutes.' -ForegroundColor DarkGray

    $bootstrap = Join-Path $env:TEMP 'vs_BuildTools.exe'
    Invoke-Download -Uri 'https://aka.ms/vs/17/release/vs_BuildTools.exe' -OutFile $bootstrap

    $vsArgs = @(
        '--quiet', '--wait', '--norestart', '--nocache',
        '--add', 'Microsoft.VisualStudio.Workload.VCTools',
        '--includeRecommended',
        '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.22621'
    )
    Write-Host '  Running bootstrapper (silent) ...' -ForegroundColor DarkGray
    $proc = Start-Process -FilePath $bootstrap -ArgumentList $vsArgs -Wait -PassThru
    # 0 = success, 3010 = success-but-reboot-required. Anything else is fatal.
    if ($proc.ExitCode -ne 0 -and $proc.ExitCode -ne 3010) {
        throw "VS Build Tools bootstrapper exited with code $($proc.ExitCode). See %TEMP%\dd_bootstrapper_*.log for details."
    }
    if ($proc.ExitCode -eq 3010) {
        Write-Warning 'VS Build Tools installed but flagged a reboot requirement. Reboot the box before running CI jobs.'
    }
    Remove-Item -LiteralPath $bootstrap -Force -ErrorAction SilentlyContinue
}

# ------------------------------------------------------------------------
# 3) Verify
# ------------------------------------------------------------------------
Write-Host ''
Write-Host '=== Verification ===' -ForegroundColor Cyan

$git = Get-Command git -ErrorAction SilentlyContinue
if ($git) {
    Write-Host "  git      : $($git.Source)" -ForegroundColor Green
} else {
    Write-Warning 'git not found on PATH after install. Retry the -SkipBuildTools path, or reboot + rerun.'
}

$bashPath = 'C:\Program Files\Git\bin\bash.exe'
if (Test-Path -LiteralPath $bashPath) {
    Write-Host "  bash     : $bashPath" -ForegroundColor Green
} else {
    Write-Warning "bash not found at $bashPath -- actions that shell to bash will fail."
}

$signtool = Get-ChildItem -Path 'C:\Program Files (x86)\Windows Kits\10\bin', 'C:\Program Files\Windows Kits\10\bin' `
              -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1 -ExpandProperty FullName
if ($signtool) {
    Write-Host "  signtool : $signtool" -ForegroundColor Green
} else {
    Write-Warning 'signtool.exe not found. VS Build Tools may still be installing; verify with the Visual Studio Installer UI.'
}

$linkExe = Get-ChildItem -Path 'C:\Program Files\Microsoft Visual Studio', 'C:\BuildTools' `
             -Recurse -Filter link.exe -ErrorAction SilentlyContinue |
           Where-Object { $_.FullName -match '\\MSVC\\.*\\bin\\Hostx64\\x64\\link\.exe$' } |
           Sort-Object FullName -Descending |
           Select-Object -First 1 -ExpandProperty FullName
if ($linkExe) {
    Write-Host "  link.exe : $linkExe" -ForegroundColor Green
} else {
    Write-Warning 'MSVC link.exe not found. Rerun without -SkipBuildTools or install manually.'
}

# ------------------------------------------------------------------------
# 4) Restart runner service if present (so it sees the fresh Machine PATH)
# ------------------------------------------------------------------------
$runnerSvc = Get-Service -Name 'actions.runner.*' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($runnerSvc) {
    Write-Host ''
    Write-Host "Restarting runner service '$($runnerSvc.Name)' so it inherits the fresh PATH ..." -ForegroundColor Cyan
    Restart-Service -Name $runnerSvc.Name -Force
    Start-Sleep 3
    $runnerSvc = Get-Service -Name $runnerSvc.Name
    Write-Host "  Service status: $($runnerSvc.Status)" -ForegroundColor Green
} else {
    Write-Host ''
    Write-Host 'No runner service detected yet. Run Install-CmtraceRunner.ps1 next.' -ForegroundColor Yellow
}

Write-Host ''
Write-Host 'Prereqs complete.' -ForegroundColor Green
