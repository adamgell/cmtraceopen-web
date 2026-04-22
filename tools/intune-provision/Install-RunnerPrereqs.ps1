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

    Steps:
        1. Elevation + TLS 1.2 sanity.
        2. winget install Git.Git --scope machine  (removes any prior
           user-scope install first so the bin dir lands in Program Files).
        3. Ensure C:\Program Files\Git\cmd + \bin on Machine PATH so the
           runner's service account inherits them.
        4. winget install Microsoft.VisualStudio.2022.BuildTools with the
           VCTools workload + Windows 11 SDK. Puts link.exe on the box
           and wires up the MSVC vcvars for Rust's MSVC target.
        5. Verifies git.exe, bash.exe, and signtool.exe are all locatable
           via the Machine PATH / known install roots.
        6. Restarts the runner service if it exists, so the runner
           process inherits the freshly-updated PATH.

.PARAMETER SkipGit
    Skip step 2/3. Useful if Git is already installed via another method.

.PARAMETER SkipBuildTools
    Skip step 4. Useful if VS 2022 or later is already installed with
    the C++ workload.

.NOTES
    Runs on Windows PowerShell 5.1 and PowerShell 7+. ASCII-only.
    Depends on winget being available (ships with Windows 11; on older
    Windows 10 hosts install App Installer from the Store).
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
function Assert-Winget {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw 'winget not found. On Windows 10 install App Installer from the Microsoft Store, or install Git + VS Build Tools manually.'
    }
}

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

function Install-WingetPackage {
    param(
        [Parameter(Mandatory)][string] $Id,
        [string] $Override
    )
    $args = @(
        'install', '--id', $Id, '-e',
        '--accept-source-agreements', '--accept-package-agreements',
        '--scope', 'machine',
        '--silent'
    )
    if ($Override) { $args += @('--override', $Override) }
    Write-Host "  winget $($args -join ' ')" -ForegroundColor DarkGray
    & winget @args
    $code = $LASTEXITCODE
    # winget returns exit 0 on fresh install, and a specific code for
    # "already installed" / "no applicable upgrade" -- treat all of
    # those as success. The actual codes are:
    #   0x00000000 -- success
    #   0x8A150077 -- already installed, no applicable upgrade
    #   0x8A15002B -- already installed (newer version)
    #   -1978335135 / 2316632161 = 0x8A150061 -- latest already installed
    if ($code -ne 0) {
        $hex = '0x{0:X8}' -f $code
        Write-Host "  winget exit $code ($hex) -- treating as 'already installed / no-op'." -ForegroundColor Yellow
    }
}

# ------------------------------------------------------------------------
# 1) Git (machine scope)
# ------------------------------------------------------------------------
if ($SkipGit) {
    Write-Host 'Skipping Git install (-SkipGit).' -ForegroundColor DarkGray
} else {
    Assert-Winget
    Write-Host 'Installing Git for Windows (machine scope) ...' -ForegroundColor Cyan
    # Remove any prior user-scope install so the machine-scope install
    # drops files at C:\Program Files\Git where services can see them.
    try {
        $userInstalled = winget list --id Git.Git --exact 2>$null
        if ($LASTEXITCODE -eq 0 -and $userInstalled -match 'Git\.Git') {
            $currentGit = Get-Command git -ErrorAction SilentlyContinue
            if ($currentGit -and $currentGit.Source -notlike 'C:\Program Files\Git\*') {
                Write-Host "  Removing non-machine-scope Git at $($currentGit.Source) ..." -ForegroundColor Yellow
                winget uninstall --id Git.Git --silent 2>$null | Out-Null
            }
        }
    } catch {
        # Non-fatal; proceed to install.
    }
    Install-WingetPackage -Id 'Git.Git'

    # The Git installer adds Program Files\Git\cmd to PATH automatically
    # but some installer versions only update User PATH. Force both
    # directories into Machine PATH so the runner service inherits them.
    Add-ToMachinePath -Dir 'C:\Program Files\Git\cmd'
    Add-ToMachinePath -Dir 'C:\Program Files\Git\bin'
}

# ------------------------------------------------------------------------
# 2) Visual Studio 2022 Build Tools (C++ workload + Windows 11 SDK)
# ------------------------------------------------------------------------
if ($SkipBuildTools) {
    Write-Host 'Skipping VS Build Tools install (-SkipBuildTools).' -ForegroundColor DarkGray
} else {
    Assert-Winget
    Write-Host 'Installing Visual Studio 2022 Build Tools (VCTools + Win11 SDK) ...' -ForegroundColor Cyan
    Write-Host '  This download is multi-GB and may take 5-10 minutes.' -ForegroundColor DarkGray
    # --override is passed verbatim to the VS installer bootstrapper.
    $vsOverride = @(
        '--quiet',
        '--wait',
        '--norestart',
        '--nocache',
        '--add', 'Microsoft.VisualStudio.Workload.VCTools',
        '--includeRecommended',
        '--add', 'Microsoft.VisualStudio.Component.Windows11SDK.22621'
    ) -join ' '
    Install-WingetPackage -Id 'Microsoft.VisualStudio.2022.BuildTools' -Override $vsOverride
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
