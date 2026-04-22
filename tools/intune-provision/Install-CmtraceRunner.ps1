<#
.SYNOPSIS
    One-shot installer for the cmtraceopen GitHub Actions self-hosted
    runner on an Intune-enrolled Windows box.

.DESCRIPTION
    Run this on the Windows machine in an **elevated** PowerShell after
    the box is Entra-joined + Intune-enrolled and the Cloud PKI
    code-signing cert has landed in LocalMachine\My.

    What it does:
        1. Verifies Entra join + Intune enrollment via dsregcmd.
        2. Verifies a code-signing cert (EKU 1.3.6.1.5.5.7.3.3) is present.
        3. Downloads the latest actions/runner release for win-x64.
        4. Configures the runner against the cmtraceopen-web repo with
           labels self-hosted,windows,cmtrace-build (runner name defaults
           to the machine's own hostname - no rename needed).
        5. Installs the runner as a Windows service and starts it.
        6. Grants NETWORK SERVICE read access to the code-signing cert's
           private key so signtool can use it from CI.

    Idempotent where possible - re-running after a successful install
    skips the download + config steps.

.PARAMETER Token
    One-time registration token from
    https://github.com/<org>/<repo>/settings/actions/runners/new.
    Tokens expire ~1 hour after generation.

.PARAMETER Repo
    GitHub URL of the repo the runner attaches to.
    Default: https://github.com/adamgell/cmtraceopen-web.

.PARAMETER InstallDir
    Where to install the runner. Default: C:\actions-runner.

.PARAMETER Labels
    Runner labels. Default: self-hosted,windows,cmtrace-build.

.PARAMETER RunnerName
    Runner display name in the GitHub UI. Defaults to the machine's
    hostname, so no rename is required.

.PARAMETER PrecheckOnly
    Run the environment + certificate precheck and exit without
    downloading or installing the runner. Useful to verify a device is
    cert-ready before grabbing a (short-lived) GitHub token.

.PARAMETER SkipCertCheck
    Bypass the cert precheck. Useful if you intentionally want to stand
    up the runner before certs have landed (agent-jobs will work; signing
    jobs will fail until the certs arrive).

.PARAMETER IssuerPattern
    Regex matched against the Issuer field of certs in LocalMachine\My
    when the precheck selects the "our" Cloud PKI client cert. Needed
    because Intune-enrolled boxes carry several Microsoft-issued
    client-auth certs that we must ignore. Default matches the Gell lab
    Cloud PKI issuing CA; override for other deployments.

.EXAMPLE
    # Precheck only - no token needed, no install performed.
    .\Install-CmtraceRunner.ps1 -PrecheckOnly

.EXAMPLE
    # Full install once the precheck passes.
    .\Install-CmtraceRunner.ps1 -Token 'AAA...XYZ'

.NOTES
    Does NOT do Entra join or Intune enrollment - those are portal /
    Settings-app tasks. Does not configure the cmtrace agent itself.

    Runs on Windows PowerShell 5.1 and PowerShell 7+. ASCII-only on
    purpose (PS 5.1 reads .ps1 files as the system code page by default,
    so non-ASCII dashes break the parser).
#>
#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]  $Token,
    [string]  $Repo       = 'https://github.com/adamgell/cmtraceopen-web',
    [string]  $InstallDir = 'C:\actions-runner',
    [string]  $Labels     = 'self-hosted,windows,cmtrace-build',
    [string]  $RunnerName = $env:COMPUTERNAME,
    [switch]  $PrecheckOnly,
    [switch]  $SkipCertCheck,
    [string]  $IssuerPattern = 'issuing\.gell\.internal\.cdw\.lab'
)

if (-not $PrecheckOnly -and -not $Token) {
    throw "-Token is required unless -PrecheckOnly is set. Generate one at $Repo/settings/actions/runners/new"
}

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Windows PowerShell 5.1 defaults SecurityProtocol to Ssl3/Tls - GitHub
# and most modern endpoints require TLS 1.2+. PowerShell 7 is already
# fine, but setting it unconditionally is harmless.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # Some .NET profiles may not expose Tls12; Invoke-RestMethod will
    # surface a clearer error below.
}

# ------------------------------------------------------------------------
# Sanity: elevated?
# ------------------------------------------------------------------------
$currentPrincipal = New-Object System.Security.Principal.WindowsPrincipal(
    [System.Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $currentPrincipal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'This script must be run in an elevated PowerShell (Run as Administrator).'
}

# ------------------------------------------------------------------------
# Precheck: Entra/Intune enrollment + both Cloud PKI certs
# ------------------------------------------------------------------------
$EkuClientAuth  = '1.3.6.1.5.5.7.3.2'
$EkuCodeSigning = '1.3.6.1.5.5.7.3.3'

# Any Intune-enrolled box has several Microsoft-issued client-auth certs
# (MS-Organization-Access, Intune MDM Device CA, etc.) that we must ignore.
# The Cloud PKI cert is distinguished by having EKU 1.3.6.1.5.5.7.3.3
# (Code Signing) AND being issued by the Cloud PKI issuing CA. Match on
# both to avoid false positives. Default regex matches the 'Gell - PKI
# Issuing' CN; override with -IssuerPattern for a different deployment.
function Get-CmtraceCert {
    param(
        [string] $EkuOid,
        [string] $IssuerPattern = 'issuing\.gell\.internal\.cdw\.lab'
    )
    Get-ChildItem Cert:\LocalMachine\My |
        Where-Object {
            $_.EnhancedKeyUsageList.ObjectId -contains $EkuOid -and
            $_.Issuer -match $IssuerPattern
        } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1
}

function Test-CertChain {
    param([System.Security.Cryptography.X509Certificates.X509Certificate2] $Cert)
    $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
    $chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::NoCheck
    $ok = $chain.Build($Cert)
    $statusMessages = @()
    foreach ($el in $chain.ChainStatus) { $statusMessages += $el.StatusInformation.Trim() }
    [pscustomobject]@{
        Ok       = $ok
        Statuses = ($statusMessages -join '; ')
        Chain    = ($chain.ChainElements |
                    ForEach-Object { $_.Certificate.Subject } ) -join ' <- '
    }
}

function Get-SanUris {
    param([System.Security.Cryptography.X509Certificates.X509Certificate2] $Cert)
    $ext = $Cert.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.17' } | Select-Object -First 1
    if (-not $ext) { return @() }
    $text = $ext.Format($true)   # multi-line
    $uris = @()
    foreach ($line in $text -split "`r?`n") {
        if ($line -match '(?i)\bURL=\s*(\S+)') { $uris += $matches[1].Trim() }
        elseif ($line -match '(?i)\bURI=\s*(\S+)') { $uris += $matches[1].Trim() }
    }
    return $uris
}

function Invoke-CmtracePrecheck {
    Write-Host '=== Precheck ===' -ForegroundColor Cyan

    # Entra join + Intune enrollment
    $dsreg = (& dsregcmd /status) -join "`n"
    if ($dsreg -notmatch 'AzureAdJoined\s*:\s*YES') {
        throw 'Device is not Entra-joined. Settings > Accounts > Access work or school > Connect > Join this device to Microsoft Entra ID.'
    }
    Write-Host '  Entra join          : OK' -ForegroundColor Green

    if ($dsreg -notmatch 'MdmUrl\s*:\s*\S') {
        throw 'Device Entra-joined but not Intune-enrolled (MdmUrl empty). Fix: Entra admin > Mobility (MDM and MAM) > Microsoft Intune > MDM user scope = All/Some, then unjoin + rejoin.'
    }
    Write-Host '  Intune enrollment   : OK' -ForegroundColor Green

    # Code-signing cert (required for CI jobs). Filter by the Cloud PKI
    # issuer so we don't accidentally match Microsoft-issued certs.
    $codeCert = Get-CmtraceCert -EkuOid $EkuCodeSigning -IssuerPattern $IssuerPattern
    if (-not $codeCert) {
        throw "Code-signing cert (EKU $EkuCodeSigning, issuer matching '$IssuerPattern') not found in LocalMachine\My. Check: device is in the build-machines group (or profile is assigned to All Devices), and Intune has synced. Override the issuer filter with -IssuerPattern if your Cloud PKI has a different CN."
    }
    Write-Host ("  Code-signing cert   : OK  ({0}, exp {1:yyyy-MM-dd})" -f $codeCert.Thumbprint, $codeCert.NotAfter) -ForegroundColor Green

    # Client-auth cert (required for agent mTLS). Prefer the same Cloud
    # PKI cert if it carries both EKUs (combined profile); otherwise look
    # for a separate client-auth-only cert from the same issuer.
    $clientCert = if ($codeCert.EnhancedKeyUsageList.ObjectId -contains $EkuClientAuth) {
        $codeCert
    } else {
        Get-CmtraceCert -EkuOid $EkuClientAuth -IssuerPattern $IssuerPattern
    }
    if (-not $clientCert) {
        throw "Client-auth cert (EKU $EkuClientAuth, issuer matching '$IssuerPattern') not found in LocalMachine\My."
    }
    Write-Host ("  Client-auth cert    : OK  ({0}, exp {1:yyyy-MM-dd})" -f $clientCert.Thumbprint, $clientCert.NotAfter) -ForegroundColor Green

    # Chain build - implicit trust-anchor check (means the 'Gell - Root
    # Trusted Cert' config actually landed in LocalMachine\Root).
    $chain = Test-CertChain -Cert $clientCert
    if (-not $chain.Ok) {
        throw "Client cert chain does not validate. $($chain.Statuses) Chain: $($chain.Chain). Fix: confirm the trusted-root device config is assigned to this device and sync has completed."
    }
    Write-Host "  Cert chain builds   : OK  ($($chain.Chain))" -ForegroundColor Green

    # SAN URI - api-server's identity parser reads device://<tenant>/<aad-device-id>.
    $uris = @(Get-SanUris -Cert $clientCert)
    $deviceUris = @($uris | Where-Object { $_ -match '^device://[^/]+/[^/]+$' })
    if ($deviceUris.Count -eq 0) {
        Write-Warning "Client cert has no SAN URI of the form device://<tenant>/<aad-device-id>. mTLS device identity will fall back to the X-Device-Id header path."
        Write-Warning 'Fix: add Subject alternative name > URI > device://<tenant-guid>/{{AAD_Device_ID}} to the SCEP profile in Intune, then re-sync.'
    } else {
        Write-Host ("  Cert SAN URI        : OK  ({0})" -f $deviceUris[0]) -ForegroundColor Green
    }

    [pscustomobject]@{
        CodeSigningCert = $codeCert
        ClientAuthCert  = $clientCert
    }
}

if ($SkipCertCheck) {
    Write-Warning 'Skipping cert precheck (-SkipCertCheck set).'
    $precheck = [pscustomobject]@{ CodeSigningCert = $null; ClientAuthCert = $null }
} else {
    $precheck = Invoke-CmtracePrecheck
}

if ($PrecheckOnly) {
    Write-Host ''
    Write-Host 'Precheck complete (-PrecheckOnly). Re-run with -Token <github-token> to install the runner.' -ForegroundColor Green
    return
}

$signCert = $precheck.CodeSigningCert

# ------------------------------------------------------------------------
# 3) Download the latest runner
# ------------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir | Out-Null
}
Push-Location $InstallDir

if (Test-Path -LiteralPath (Join-Path $InstallDir 'config.cmd')) {
    Write-Host "Runner already extracted at $InstallDir - skipping download." -ForegroundColor DarkGray
} else {
    Write-Host 'Resolving latest actions/runner release ...' -ForegroundColor Cyan
    $release = Invoke-RestMethod 'https://api.github.com/repos/actions/runner/releases/latest' `
        -Headers @{ 'User-Agent' = 'cmtraceopen-provisioner' }
    $asset = $release.assets | Where-Object { $_.name -match '^actions-runner-win-x64-.*\.zip$' } | Select-Object -First 1
    if (-not $asset) { throw 'Could not find a win-x64 runner asset on the latest release.' }
    Write-Host "  Downloading $($asset.name) ..." -ForegroundColor Green
    $zipPath = Join-Path $InstallDir 'actions-runner.zip'
    # PS 5.1's Invoke-WebRequest progress UI is ~100x slower than raw
    # throughput because it re-renders per buffer. Silence it for the
    # duration of the download.
    $savedProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -UseBasicParsing
    } finally {
        $ProgressPreference = $savedProgress
    }
    Expand-Archive -Path $zipPath -DestinationPath $InstallDir -Force
    Remove-Item $zipPath -Force
}

# ------------------------------------------------------------------------
# 4) Configure runner
# ------------------------------------------------------------------------
if (Test-Path -LiteralPath (Join-Path $InstallDir '.runner')) {
    Write-Host 'Runner already configured - skipping config.cmd.' -ForegroundColor DarkGray
} else {
    Write-Host "Registering runner '$RunnerName' against $Repo ..." -ForegroundColor Cyan
    & .\config.cmd --unattended `
        --url $Repo `
        --token $Token `
        --name $RunnerName `
        --labels $Labels `
        --replace
    if ($LASTEXITCODE -ne 0) { throw "config.cmd failed with exit code $LASTEXITCODE." }
}

# ------------------------------------------------------------------------
# 5) Install + start the Windows service
#
# The Windows runner zip ships no svc.cmd (that's a Linux/macOS artifact).
# config.cmd --runasservice can create it, but only during initial config;
# for already-configured runners we install via sc.exe directly, which is
# also what --runasservice does under the hood.
# ------------------------------------------------------------------------
$repoSegment = ($Repo -replace '^https?://github\.com/', '') -replace '/', '-'
$svcName     = "actions.runner.$repoSegment.$RunnerName"

$svc = Get-Service -Name $svcName -ErrorAction SilentlyContinue
if (-not $svc) {
    Write-Host "Creating Windows service '$svcName' ..." -ForegroundColor Cyan
    # sc.exe quirk: `key=` tokens (binPath=, start=, obj=) MUST be a
    # separate argv token from their values, with a literal space between.
    # PowerShell's call operator passes each array element as its own argv
    # entry and quotes only values with whitespace, which is exactly what
    # sc.exe wants. Using cmd.exe /c here breaks the quoting.
    $runnerExe    = Join-Path $InstallDir 'bin\Runner.Listener.exe'
    $binPathValue = "$runnerExe run"
    Write-Host "  binPath= $binPathValue" -ForegroundColor DarkGray
    & sc.exe create $svcName 'binPath=' $binPathValue 'start=' 'auto' 'obj=' 'NT AUTHORITY\NETWORK SERVICE'
    if ($LASTEXITCODE -ne 0) { throw "sc.exe create failed with exit code $LASTEXITCODE." }
    & sc.exe config $svcName 'DisplayName=' "GitHub Actions Runner ($RunnerName)" | Out-Null
    & sc.exe description $svcName 'cmtraceopen self-hosted GitHub Actions runner' | Out-Null
    $svc = Get-Service -Name $svcName -ErrorAction Stop
} else {
    Write-Host "Service '$svcName' already exists." -ForegroundColor DarkGray
}

if ($svc.Status -ne 'Running') {
    Write-Host "Starting service '$svcName' ..." -ForegroundColor Cyan
    Start-Service -Name $svcName
} else {
    Write-Host "Service '$svcName' is already Running." -ForegroundColor DarkGray
}

Pop-Location

# ------------------------------------------------------------------------
# 6) Grant NETWORK SERVICE read on the code-signing cert's private key
# ------------------------------------------------------------------------
if ($signCert) {
    Write-Host 'Granting NETWORK SERVICE read access to the code-signing private key ...' -ForegroundColor Cyan
    try {
        $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($signCert)
        if ($null -eq $rsa) {
            Write-Warning 'Could not open the RSA private key (non-RSA cert?). Grant read access manually via certlm.msc > Manage Private Keys.'
        } else {
            # CNG-backed keys expose a file path; ACL it directly.
            if ($rsa -is [System.Security.Cryptography.RSACng]) {
                $keyName = $rsa.Key.UniqueName
                $keyPath = Join-Path $env:ProgramData 'Microsoft\Crypto\Keys' $keyName
                if (-not (Test-Path -LiteralPath $keyPath)) {
                    $keyPath = Join-Path $env:ProgramData "Microsoft\Crypto\SystemKeys" $keyName
                }
                if (Test-Path -LiteralPath $keyPath) {
                    $acl = Get-Acl -Path $keyPath
                    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
                        'NT AUTHORITY\NETWORK SERVICE', 'Read', 'Allow')
                    $acl.AddAccessRule($rule)
                    Set-Acl -Path $keyPath -AclObject $acl
                    Write-Host "  Granted NETWORK SERVICE read on $keyPath." -ForegroundColor Green
                } else {
                    Write-Warning "CNG key file not found for thumbprint $($signCert.Thumbprint). Grant manually via certlm.msc."
                }
            } else {
                Write-Warning 'Legacy CAPI key detected - use certlm.msc > Manage Private Keys to grant NETWORK SERVICE read.'
            }
        }
    } catch {
        Write-Warning "Private-key ACL update failed: $($_.Exception.Message). Grant manually via certlm.msc."
    }
}

Write-Host ''
Write-Host 'Runner install complete.' -ForegroundColor Green
Write-Host "Confirm at $Repo/settings/actions/runners - '$RunnerName' should show Idle with labels [$Labels]."
