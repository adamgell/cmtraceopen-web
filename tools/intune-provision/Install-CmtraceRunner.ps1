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

.PARAMETER ServiceAccount
    Local Windows account the runner service runs as. Must be a real
    account with a profile - NOT NT AUTHORITY\NETWORK SERVICE (see
    below). Default: cmtraceopen-runner, a local account the script
    creates on first run with a randomly generated password.

    Why not NETWORK SERVICE? It has no USERPROFILE / HOME, so git can't
    find .gitconfig / credential helpers / SSH known_hosts, and
    Intune-style credential-helper prompts fail in non-interactive
    contexts. A dedicated local account gets a real profile and "just
    works" for git + submodules + signtool.

.PARAMETER ServicePassword
    SecureString password for the service account. If omitted and the
    account doesn't exist yet, a random 24-char password is generated.
    You don't normally need to reuse it - once config.cmd --runasservice
    has registered the service, the password lives in the LSA secret
    store and isn't referenced again.

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
    [string]  $IssuerPattern = 'issuing\.gell\.internal\.cdw\.lab',
    [string]  $ServiceAccount = 'cmtraceopen-runner',
    [securestring] $ServicePassword,
    [switch]  $SkipSelfUpdate
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
# Self-update: fetch the latest copy of this script from main and re-exec
# if it differs from what's on disk. Keeps a runner host from silently
# running stale logic after we push a fix. Opt out with -SkipSelfUpdate.
# ------------------------------------------------------------------------
if (-not $SkipSelfUpdate -and $MyInvocation.MyCommand.Path) {
    $selfPath   = $MyInvocation.MyCommand.Path
    $latestUri  = "https://raw.githubusercontent.com/adamgell/cmtraceopen-web/main/tools/intune-provision/Install-CmtraceRunner.ps1?t=$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"
    $latestTmp  = [IO.Path]::Combine([IO.Path]::GetDirectoryName($selfPath), 'Install-CmtraceRunner.ps1.new')
    $savedProg  = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $latestUri -OutFile $latestTmp -UseBasicParsing -ErrorAction Stop
    } catch {
        Write-Warning "Self-update check failed ($($_.Exception.Message)); continuing with local copy."
        $latestTmp = $null
    } finally {
        $ProgressPreference = $savedProg
    }
    if ($latestTmp -and (Test-Path -LiteralPath $latestTmp)) {
        $localHash  = (Get-FileHash -Path $selfPath   -Algorithm SHA256).Hash
        $latestHash = (Get-FileHash -Path $latestTmp -Algorithm SHA256).Hash
        if ($localHash -ne $latestHash) {
            Write-Host "Self-updating from main (local $($localHash.Substring(0,8)) -> latest $($latestHash.Substring(0,8))) ..." -ForegroundColor Cyan
            Move-Item -LiteralPath $latestTmp -Destination $selfPath -Force
            # Re-exec with all original params + -SkipSelfUpdate so we
            # don't loop. pwsh / powershell inherit the same interpreter.
            $forwardedParams = @{} + $PSBoundParameters
            $forwardedParams['SkipSelfUpdate'] = $true
            & $selfPath @forwardedParams
            exit $LASTEXITCODE
        } else {
            Remove-Item -LiteralPath $latestTmp -Force -ErrorAction SilentlyContinue
            Write-Host 'Self-update check: already on latest.' -ForegroundColor DarkGray
        }
    }
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
# Local service account helpers
# ------------------------------------------------------------------------
function New-RandomPassword {
    param([int] $Length = 24)
    # Mix of upper/lower/digit/symbol, length 24 -- comfortably meets any
    # local password policy without needing runtime tuning.
    $upper = [char[]](65..90)
    $lower = [char[]](97..122)
    $digit = [char[]](48..57)
    $sym   = [char[]]'!@#$%^&*()-_=+[]{}'
    $all   = $upper + $lower + $digit + $sym
    $rng   = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $bytes = New-Object byte[] $Length
    $rng.GetBytes($bytes)
    # Require at least one of each class to satisfy complexity policies.
    $chars = @(
        $upper[$bytes[0] % $upper.Length],
        $lower[$bytes[1] % $lower.Length],
        $digit[$bytes[2] % $digit.Length],
        $sym[  $bytes[3] % $sym.Length]
    )
    for ($i = 4; $i -lt $Length; $i++) {
        $chars += $all[$bytes[$i] % $all.Length]
    }
    ($chars | Sort-Object { Get-Random }) -join ''
}

function Initialize-CmtraceServiceAccount {
    param(
        [Parameter(Mandatory)] [string] $AccountName,
        [securestring] $Password
    )
    $existing = Get-LocalUser -Name $AccountName -ErrorAction SilentlyContinue
    if ($existing) {
        Write-Host "Service account '$AccountName' exists; reusing." -ForegroundColor DarkGray
        # We don't know the existing password. If the caller didn't pass
        # one either, they'll need to rerun with -ServicePassword OR
        # reset the password here. For idempotency we reset to a new
        # random value so the install always succeeds.
        if (-not $Password) {
            $pt  = New-RandomPassword
            $Password = ConvertTo-SecureString -String $pt -AsPlainText -Force
            Set-LocalUser -Name $AccountName -Password $Password
            Write-Host "  Password rotated (ephemeral -- used only for service install)." -ForegroundColor DarkGray
        }
    } else {
        if (-not $Password) {
            $pt = New-RandomPassword
            $Password = ConvertTo-SecureString -String $pt -AsPlainText -Force
        }
        Write-Host "Creating local account '$AccountName' ..." -ForegroundColor Green
        # New-LocalUser caps Description at 48 chars on Windows.
        New-LocalUser -Name $AccountName `
                      -Password $Password `
                      -FullName 'CMTraceOpen GitHub Runner' `
                      -Description 'cmtraceopen-web self-hosted runner account' `
                      -PasswordNeverExpires `
                      -AccountNeverExpires `
                      -UserMayNotChangePassword | Out-Null
    }
    # Ensure the account is a member of the built-in Users group so it
    # has a profile directory and basic read access to its own files.
    # Also add to Administrators? No -- signtool + code-signing cert
    # access only needs ACLs on the specific private key, not admin.
    try {
        Add-LocalGroupMember -Group 'Users' -Member $AccountName -ErrorAction Stop
    } catch [Microsoft.PowerShell.Commands.MemberExistsException] {
        # already a member -- ok
    } catch {
        # On domain-joined boxes Add-LocalGroupMember can throw NotFound
        # if the group name is localized. Best-effort; carry on.
        Write-Warning "Could not add $AccountName to Users group: $($_.Exception.Message)"
    }
    return $Password
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
# 4) Configure runner + install as a Windows service in one step
#
# config.cmd --runasservice:
#   - registers the runner with GitHub
#   - creates the Windows service (actions.runner.<owner>-<repo>.<name>)
#   - re-encrypts the runner's private credentials with machine-scope
#     DPAPI so NETWORK SERVICE can decrypt them at service-start time
#
# Skipping --runasservice means the credentials are encrypted with
# user-scope DPAPI, the service is never created, and any later manual
# sc.exe create call produces a service that can't actually start.
# ------------------------------------------------------------------------
if (Test-Path -LiteralPath (Join-Path $InstallDir '.runner')) {
    Write-Host 'Runner already configured - skipping config.cmd.' -ForegroundColor DarkGray
    Write-Host '  (To reconfigure: .\config.cmd remove  then rerun this script.)' -ForegroundColor DarkGray
} else {
    Write-Host "Ensuring local service account '$ServiceAccount' exists ..." -ForegroundColor Cyan
    $ServicePassword = Initialize-CmtraceServiceAccount -AccountName $ServiceAccount -Password $ServicePassword
    $ptPassword = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
        [Runtime.InteropServices.Marshal]::SecureStringToBSTR($ServicePassword)
    )

    Write-Host "Registering runner '$RunnerName' against $Repo ..." -ForegroundColor Cyan
    & .\config.cmd --unattended `
        --url $Repo `
        --token $Token `
        --name $RunnerName `
        --labels $Labels `
        --replace `
        --runasservice `
        --windowslogonaccount ".\$ServiceAccount" `
        --windowslogonpassword $ptPassword
    $cfgExit = $LASTEXITCODE
    # Wipe the plaintext password from memory ASAP regardless of outcome.
    $ptPassword = $null
    if ($cfgExit -ne 0) { throw "config.cmd failed with exit code $cfgExit." }
}

# ------------------------------------------------------------------------
# 5) Verify the service exists and is running
#
# config.cmd --runasservice (above) creates + starts the service on our
# behalf. We just sanity-check it ended up in the Running state so the
# caller sees a clear diagnostic if something went sideways.
# ------------------------------------------------------------------------
$repoSegment = ($Repo -replace '^https?://github\.com/', '') -replace '/', '-'
$svcName     = "actions.runner.$repoSegment.$RunnerName"

$svc = Get-Service -Name $svcName -ErrorAction SilentlyContinue
if (-not $svc) {
    throw "Service '$svcName' was not created. Inspect C:\actions-runner\_diag\ for errors from config.cmd."
}
if ($svc.Status -ne 'Running') {
    Write-Host "Starting service '$svcName' ..." -ForegroundColor Cyan
    Start-Service -Name $svcName
} else {
    Write-Host "Service '$svcName' is Running." -ForegroundColor DarkGray
}

Pop-Location

# ------------------------------------------------------------------------
# 6) Grant the service account read on the code-signing cert's private key
# ------------------------------------------------------------------------
if ($signCert) {
    $aclPrincipal = ".\$ServiceAccount"
    Write-Host "Granting $aclPrincipal read access to the code-signing private key ..." -ForegroundColor Cyan
    try {
        $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($signCert)
        if ($null -eq $rsa) {
            Write-Warning 'Could not open the RSA private key (non-RSA cert?). Grant read access manually via certlm.msc > Manage Private Keys.'
        } else {
            # CNG-backed keys expose a file path; ACL it directly.
            if ($rsa -is [System.Security.Cryptography.RSACng]) {
                # UniqueName may be:
                #   - a bare filename (Microsoft Software KSP): resolve
                #     against Microsoft\Crypto\Keys or SystemKeys
                #   - a full absolute path (PCP / TPM-backed keys):
                #     use it verbatim
                $keyName = $rsa.Key.UniqueName
                $keyPath = $null
                if ([IO.Path]::IsPathRooted($keyName)) {
                    if (Test-Path -LiteralPath $keyName) { $keyPath = $keyName }
                } else {
                    $candidates = @(
                        (Join-Path $env:ProgramData 'Microsoft\Crypto\Keys'),
                        (Join-Path $env:ProgramData 'Microsoft\Crypto\SystemKeys'),
                        (Join-Path $env:ProgramData 'Microsoft\Crypto\PCPKSP')
                    )
                    foreach ($base in $candidates) {
                        $try = Join-Path $base $keyName
                        if (Test-Path -LiteralPath $try) { $keyPath = $try; break }
                        if (Test-Path -LiteralPath "$try.PCPKEY") { $keyPath = "$try.PCPKEY"; break }
                    }
                }
                if ($keyPath) {
                    $acl = Get-Acl -Path $keyPath
                    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
                        $aclPrincipal, 'Read', 'Allow')
                    $acl.AddAccessRule($rule)
                    Set-Acl -Path $keyPath -AclObject $acl
                    Write-Host "  Granted $aclPrincipal read on $keyPath." -ForegroundColor Green
                } else {
                    Write-Warning "CNG key file not found for thumbprint $($signCert.Thumbprint) (UniqueName '$keyName'). Grant manually via certlm.msc."
                }
            } else {
                Write-Warning 'Legacy CAPI key detected - use certlm.msc > Manage Private Keys to grant the service account read access.'
            }
        }
    } catch {
        Write-Warning "Private-key ACL update failed: $($_.Exception.Message). Grant manually via certlm.msc."
    }
}

Write-Host ''
Write-Host 'Runner install complete.' -ForegroundColor Green
Write-Host "Confirm at $Repo/settings/actions/runners - '$RunnerName' should show Idle with labels [$Labels]."
