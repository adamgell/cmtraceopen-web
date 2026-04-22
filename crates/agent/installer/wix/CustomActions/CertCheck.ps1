# CertCheck.ps1 — Cloud PKI client certificate presence check.
#
# Runs as a deferred MSI custom action during installation of CMTraceOpenAgent.
# Invoked by Product.wxs after InstallFiles and before StartServices so the
# result lands in the MSI log next to the service-start step.
#
# IMPORTANT: This script always exits 0. A missing cert is a WARNING, not a
# failure — the cert may arrive minutes later on the next Intune sync. A
# hard-fail here would block fleet rollouts that interleave MSI deployment
# with cert-profile assignment. See design doc §6 for the full rationale.
#
# Output goes to STDOUT. In a deferred MSI custom action the WiX runner
# captures STDOUT/STDERR into the MSI log (visible with /l*v install.log).
#
# Invoked by msiexec as:
#   powershell.exe -NonInteractive -NoProfile -ExecutionPolicy Bypass -File CertCheck.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

# Issuer CN pattern — matches the Cloud PKI Issuing CA configured in
# docs/provisioning/03-intune-cloud-pki.md. Substring match so a CA renewal
# (same CN, new serial) still gets picked up automatically.
$pattern = 'issuing.gell.internal.cdw.lab'

try {
    $found = Get-ChildItem -Path 'Cert:\LocalMachine\My' -ErrorAction Stop |
        Where-Object { $_.Issuer -match [regex]::Escape($pattern) }

    if ($found) {
        $count = @($found).Count
        Write-Output "OK: found $count Cloud PKI cert(s) matching issuer pattern '$pattern'."
        foreach ($cert in $found) {
            Write-Output "  Subject: $($cert.Subject)"
            Write-Output "  Issuer:  $($cert.Issuer)"
            Write-Output "  Thumbprint: $($cert.Thumbprint)"
            Write-Output "  NotAfter: $($cert.NotAfter)"
        }
    } else {
        Write-Output "WARN: no client cert matching issuer pattern '$pattern' in LocalMachine\My."
        Write-Output "WARN: the CMTrace Open Agent will start, but mTLS calls to the api-server"
        Write-Output "WARN: will fail until the cert arrives via Intune Cloud PKI sync."
        Write-Output "WARN: confirm the Intune Cloud PKI cert profile is assigned to this device."
        Write-Output "WARN: typical cert-profile delivery lag is a few minutes to a few hours."
    }
} catch {
    # Accessing Cert:\ can fail in unusual restricted environments. Treat as
    # a soft warning so the install still completes.
    Write-Output "WARN: cert check failed with error: $_"
    Write-Output "WARN: unable to verify Cloud PKI cert presence."
    Write-Output "WARN: the agent will still be installed; verify cert status manually."
}

exit 0
