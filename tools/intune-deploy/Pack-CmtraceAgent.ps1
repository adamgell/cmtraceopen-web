<#
.SYNOPSIS
    Packs a folder containing the cmtraceopen-agent MSI into an Intune .intunewin payload.

.DESCRIPTION
    Wave 4 packaging helper. Wraps Microsoft's Win32 Content Prep Tool
    (IntuneWinAppUtil.exe). If the tool isn't on PATH, downloads the official binary
    from the Microsoft-Win32-Content-Prep-Tool repo to tools/intune-deploy/.bin/ and
    verifies its SHA256 against a pinned hash.

    The output .intunewin is an encrypted zip whose Detection.xml carries the AES key
    that Deploy-CmtraceAgent.ps1 uses when committing the upload to Intune.

.PARAMETER SourceFolder
    Folder containing the MSI (and any side-by-side support files). All contents are
    packed; the MSI must be at the root of this folder.

.PARAMETER SetupFile
    The MSI filename inside SourceFolder. Default: 'CMTraceOpenAgent.msi'.

.PARAMETER OutputFolder
    Folder to write the .intunewin into. Created if missing.

.PARAMETER ToolPath
    Optional override. Absolute path to IntuneWinAppUtil.exe. If unset, the script
    looks on PATH, then falls back to the cached copy under .bin/.

.PARAMETER Force
    Overwrite an existing .intunewin in OutputFolder.

.NOTES
    Prereqs:
      * PowerShell 7+
      * Network access to github.com (only on first run, when fetching the tool)

    Output: prints the absolute path of the produced .intunewin, which is the value
    you pass to Deploy-CmtraceAgent.ps1 -IntuneWinPath.

.EXAMPLE
    pwsh ./Pack-CmtraceAgent.ps1 `
        -SourceFolder 'C:\build\msi-staging' `
        -OutputFolder 'C:\build\out'

.LINK
    https://github.com/microsoft/Microsoft-Win32-Content-Prep-Tool
.LINK
    https://learn.microsoft.com/mem/intune/apps/apps-win32-prepare
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourceFolder,

    [string]$SetupFile = 'CMTraceOpenAgent.msi',

    [Parameter(Mandatory = $true)]
    [string]$OutputFolder,

    [string]$ToolPath,

    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ToolUrl = 'https://github.com/microsoft/Microsoft-Win32-Content-Prep-Tool/raw/master/IntuneWinAppUtil.exe'

# TODO pin SHA256 — Microsoft does not publish a hash on the GitHub release page
# (the binary lives at HEAD of master, not as a tagged release). Pin to whatever
# `Get-FileHash -Algorithm SHA256` reports the first time you run this script in CI,
# then bump on review when Microsoft rebuilds. See:
#   https://github.com/microsoft/Microsoft-Win32-Content-Prep-Tool/blob/master/IntuneWinAppUtil.exe
$ToolSha256 = $null  # e.g. 'A1B2C3...' — leave $null to skip the check (with a warning).

function Resolve-AbsolutePath {
    param([string]$Path)
    return [System.IO.Path]::GetFullPath((Join-Path -Path (Get-Location) -ChildPath $Path))
}

function Get-IntuneWinAppUtil {
    if ($ToolPath) {
        if (-not (Test-Path -LiteralPath $ToolPath)) {
            throw "Explicit -ToolPath not found: $ToolPath"
        }
        return (Resolve-AbsolutePath $ToolPath)
    }

    $onPath = Get-Command -Name 'IntuneWinAppUtil.exe' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $binDir = Join-Path -Path $PSScriptRoot -ChildPath '.bin'
    $cached = Join-Path -Path $binDir -ChildPath 'IntuneWinAppUtil.exe'
    if (Test-Path -LiteralPath $cached) { return (Resolve-AbsolutePath $cached) }

    Write-Host "==> Downloading IntuneWinAppUtil.exe" -ForegroundColor Cyan
    Write-Host "    from: $ToolUrl"
    Write-Host "    to  : $cached"
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    Invoke-WebRequest -Uri $ToolUrl -OutFile $cached -UseBasicParsing -ErrorAction Stop

    if ($ToolSha256) {
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $cached).Hash.ToUpperInvariant()
        $expected = $ToolSha256.ToUpperInvariant()
        if ($actual -ne $expected) {
            Remove-Item -LiteralPath $cached -Force
            throw "IntuneWinAppUtil.exe SHA256 mismatch. expected=$expected actual=$actual — refusing to use."
        }
        Write-Host "    sha256 verified: $actual" -ForegroundColor Green
    }
    else {
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $cached).Hash.ToUpperInvariant()
        Write-Warning "ToolSha256 is not pinned. Downloaded SHA256: $actual"
        Write-Warning "Pin this hash in Pack-CmtraceAgent.ps1 (`$ToolSha256`) on a trusted machine."
    }

    return (Resolve-AbsolutePath $cached)
}

# ---------- main ----------

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw "PowerShell 7+ required. Detected: $($PSVersionTable.PSVersion)"
}

if (-not (Test-Path -LiteralPath $SourceFolder -PathType Container)) {
    throw "SourceFolder not found or not a directory: $SourceFolder"
}

$SourceFolder = Resolve-AbsolutePath $SourceFolder
$OutputFolder = Resolve-AbsolutePath $OutputFolder

$msiPath = Join-Path -Path $SourceFolder -ChildPath $SetupFile
if (-not (Test-Path -LiteralPath $msiPath)) {
    throw "Setup file not found at root of SourceFolder: $msiPath"
}

if (-not (Test-Path -LiteralPath $OutputFolder)) {
    New-Item -ItemType Directory -Path $OutputFolder -Force | Out-Null
}

$expectedOutput = Join-Path -Path $OutputFolder -ChildPath ([System.IO.Path]::ChangeExtension($SetupFile, '.intunewin'))
if ((Test-Path -LiteralPath $expectedOutput) -and -not $Force) {
    throw "Output already exists: $expectedOutput. Re-run with -Force to overwrite."
}
elseif (Test-Path -LiteralPath $expectedOutput) {
    Remove-Item -LiteralPath $expectedOutput -Force
}

$tool = Get-IntuneWinAppUtil

Write-Host ''
Write-Host "==> Packing .intunewin" -ForegroundColor Cyan
Write-Host "    tool   : $tool"
Write-Host "    source : $SourceFolder"
Write-Host "    setup  : $SetupFile"
Write-Host "    output : $OutputFolder"

# IntuneWinAppUtil.exe is interactive by default unless -q is passed.
& $tool -c $SourceFolder -s $SetupFile -o $OutputFolder -q
$exit = $LASTEXITCODE
if ($exit -ne 0) {
    throw "IntuneWinAppUtil.exe exited with code $exit"
}

if (-not (Test-Path -LiteralPath $expectedOutput)) {
    throw "Expected output not produced: $expectedOutput"
}

$size = (Get-Item -LiteralPath $expectedOutput).Length
Write-Host ''
Write-Host "==> Done." -ForegroundColor Green
Write-Host "    intunewin: $expectedOutput"
Write-Host "    size     : $size bytes"
Write-Host ''
Write-Host "Pass this path to Deploy-CmtraceAgent.ps1 -IntuneWinPath."
Write-Output $expectedOutput
