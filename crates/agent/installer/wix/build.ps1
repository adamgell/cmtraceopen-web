<#
.SYNOPSIS
    Builds CMTraceOpenAgent.msi using WiX v4.

.DESCRIPTION
    Thin wrapper around `wix build` that threads the release binary path and
    version number through to the WiX preprocessor, then optionally signs the
    resulting MSI with signtool.

    Prerequisite: WiX v4 dotnet tool installed globally.
        dotnet tool install --global wix
        wix extension add WixToolset.Util.wixext
        wix extension add WixToolset.UI.wixext

.PARAMETER ReleaseBinary
    Path to the compiled cmtraceopen-agent.exe. Must exist before this script
    is invoked. CI builds this with:
        cargo build -p agent --release --target x86_64-pc-windows-msvc

.PARAMETER Version
    Semver version string (e.g. "0.1.0") to embed in the MSI ProductVersion
    and output filename. Must match the three-part MSI version format
    (Major.Minor.Patch); a fourth Revision component of 0 is appended
    automatically by WiX.

    If omitted, the script reads the version from crates/agent/Cargo.toml.

.PARAMETER SignCertThumbprint
    SHA-1 thumbprint of an Authenticode code-signing cert in CurrentUser\My.
    When provided, the script runs `signtool sign` on the finished MSI.
    Omit (or pass empty string) to skip signing — acceptable for local dev
    and pilot builds; not acceptable for GA release.

.PARAMETER OutDir
    Directory where the finished MSI is written. Created if it doesn't exist.
    Defaults to ./out relative to this script's location.

.EXAMPLE
    # Build unsigned MSI using a local cargo release binary:
    ./build.ps1 -ReleaseBinary .\target\release\cmtraceopen-agent.exe -Version 0.1.0

.EXAMPLE
    # Build + sign MSI in CI:
    ./build.ps1 `
      -ReleaseBinary target/x86_64-pc-windows-msvc/release/cmtraceopen-agent.exe `
      -Version 0.2.0 `
      -SignCertThumbprint $env:SIGN_CERT_THUMBPRINT

.NOTES
    The output file is named CMTraceOpenAgent-<Version>.msi.
    UpgradeCode is fixed at 463FD20A-1029-448F-AE5B-F81C818861D0 (Variables.wxi).
    ProductCode is auto-generated per build (Product.wxs Id="*").
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ReleaseBinary,

    [Parameter()]
    [string]$Version,

    [Parameter()]
    [string]$SignCertThumbprint,

    [Parameter()]
    [string]$OutDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Locate this script's directory — all WiX source paths are relative to it.
# ---------------------------------------------------------------------------
$ScriptDir = $PSScriptRoot
if (-not $ScriptDir) {
    # Fallback when dot-sourced or run in some non-standard hosts.
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}

# ---------------------------------------------------------------------------
# Resolve output directory.
# ---------------------------------------------------------------------------
if (-not $OutDir) {
    $OutDir = Join-Path $ScriptDir 'out'
}
if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}

# ---------------------------------------------------------------------------
# Resolve version — read from Cargo.toml if not supplied.
# ---------------------------------------------------------------------------
if (-not $Version) {
    # Walk up from the script dir to the agent crate root (two directories up:
    # wix/ -> installer/ -> agent/).
    $CargoToml = Join-Path $ScriptDir '..\..\Cargo.toml'
    if (-not (Test-Path $CargoToml)) {
        Write-Error "Cannot find crates/agent/Cargo.toml at '$CargoToml'. Pass -Version explicitly."
    }
    $VersionLine = Select-String -Path $CargoToml -Pattern '^version\s*=' |
        Select-Object -First 1
    if (-not $VersionLine) {
        Write-Error "No 'version = ...' line found in $CargoToml."
    }
    $Version = $VersionLine.Line -replace '.*=\s*"([^"]+)".*', '$1'
    Write-Verbose "Read version '$Version' from $CargoToml"
}

# Basic semver format sanity check — WiX accepts Major.Minor.Patch[.Revision].
if ($Version -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    Write-Error "Version '$Version' is not a valid MSI version (expected Major.Minor.Patch)."
}

# ---------------------------------------------------------------------------
# Validate the release binary.
# ---------------------------------------------------------------------------
if (-not (Test-Path $ReleaseBinary)) {
    Write-Error "Release binary not found: '$ReleaseBinary'. Build the agent first."
}
$ReleaseBinaryFull = Resolve-Path $ReleaseBinary

# ---------------------------------------------------------------------------
# Check wix.exe is available.
# ---------------------------------------------------------------------------
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    Write-Error @'
wix.exe not found. Install WiX v4 with:
    dotnet tool install --global wix
    wix extension add WixToolset.Util.wixext
    wix extension add WixToolset.UI.wixext
'@
}

# ---------------------------------------------------------------------------
# Build the MSI.
# ---------------------------------------------------------------------------
$OutFile = Join-Path $OutDir "CMTraceOpenAgent-$Version.msi"

$WxsFiles = @(
    'Variables.wxi',
    'Product.wxs',
    'Files.wxs',
    'Service.wxs',
    'Config.wxs'
) | ForEach-Object { Join-Path $ScriptDir $_ }

# Verify all source files exist before invoking wix.
foreach ($f in $WxsFiles) {
    if (-not (Test-Path $f)) {
        Write-Error "WiX source file not found: $f"
    }
}

Write-Host "Building CMTraceOpenAgent-$Version.msi ..."
Write-Host "  Binary:  $ReleaseBinaryFull"
Write-Host "  Output:  $OutFile"

# wix build arguments:
#   -arch x64            target 64-bit MSI (agent is x86_64 only)
#   -out <path>          output MSI path
#   -d ReleaseBinary=... path to the agent EXE (preprocessor define)
#   -d Version=...       semver for ProductVersion (preprocessor define)
#   -ext <ext>           load WiX extensions
$wixArgs = @(
    'build',
    '-arch', 'x64',
    '-out', $OutFile,
    '-d', "ReleaseBinary=$ReleaseBinaryFull",
    '-d', "Version=$Version",
    '-ext', 'WixToolset.Util.wixext'
) + $WxsFiles

& wix @wixArgs
if ($LASTEXITCODE -ne 0) {
    Write-Error "wix build failed with exit code $LASTEXITCODE."
}

Write-Host "MSI built successfully: $OutFile"

# ---------------------------------------------------------------------------
# Optional: sign the MSI.
# ---------------------------------------------------------------------------
if ($SignCertThumbprint) {
    Write-Host "Signing MSI with cert thumbprint $SignCertThumbprint ..."

    if (-not (Get-Command signtool -ErrorAction SilentlyContinue)) {
        Write-Warning "signtool.exe not found in PATH. Skipping signing."
        Write-Warning "Install the Windows SDK or add signtool to PATH."
    } else {
        & signtool sign `
            /sha1 $SignCertThumbprint `
            /tr http://timestamp.digicert.com `
            /td sha256 `
            /fd sha256 `
            /a `
            $OutFile

        if ($LASTEXITCODE -ne 0) {
            Write-Error "signtool sign failed with exit code $LASTEXITCODE."
        }

        Write-Host "MSI signed successfully."
    }
} else {
    Write-Host "No -SignCertThumbprint provided — MSI is unsigned."
    Write-Host "(Acceptable for local dev and pilot; sign before GA release.)"
}

Write-Host ""
Write-Host "Done. Output: $OutFile"
