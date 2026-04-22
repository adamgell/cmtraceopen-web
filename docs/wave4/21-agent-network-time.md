# 21 — Agent Time Sync & Network Policy Requirements

Covers the two silent-failure categories that most often trip up a first
deployment: **clock skew** that breaks TLS handshakes, and **missing
firewall rules** that block the agent from reaching the api-server or
renewing its Cloud PKI certificate.

> **Series context.** This is doc 21 in `docs/wave4/`. Doc 03
> ([`03-beta-pilot-runbook.md`](./03-beta-pilot-runbook.md)) has a
> brief Appendix A.3 on time skew and Appendix A.2 on network
> reachability; this doc is the deep-dive reference for both.

---

## Contents

1. [Time Sync](#1-time-sync)
   - [Why it matters](#11-why-it-matters)
   - [Default configuration (Intune-managed devices)](#12-default-configuration-intune-managed-devices)
   - [Verifying sync status](#13-verifying-sync-status)
   - [Failure modes and remediation](#14-failure-modes-and-remediation)
2. [Network Policy](#2-network-policy)
   - [Outbound requirements](#21-outbound-requirements)
   - [Inbound requirements](#22-inbound-requirements)
   - [DNS requirements](#23-dns-requirements)
   - [Firewall rule shapes](#24-firewall-rule-shapes)
3. [Cross-references](#3-cross-references)

---

## 1. Time Sync

### 1.1 Why it matters

Every TLS handshake validates the `notBefore` and `notAfter` fields of
the server certificate **and** of the client certificate. If the
device's clock is more than ~5 minutes off UTC, the peer will reject
the certificate with a message such as:

```
bad certificate
certificate not yet valid
certificate has expired
```

These error strings look like a *certificate problem* — wrong issuer,
wrong EKU, missing chain — but the actual root cause is a **clock
skew** of more than the TLS implementation's tolerance window (5 min
is the RFC 5246 / RFC 8446 implementation default in most TLS stacks).

The agent is also the clock source for bundle timestamps. A skewed
clock means session timestamps are wrong in the UI, making it
impossible to correlate agent events with real-world timelines.

### 1.2 Default configuration (Intune-managed devices)

Windows Time Service (`w32time`) is the built-in NTP implementation on
every supported Windows SKU. On **Intune-managed, Entra-joined**
devices the service is automatically configured to:

1. Sync to the Entra-joined tenant's domain infrastructure, **or**
2. Fall back to `time.windows.com` (Microsoft's public NTP pool) when
   not domain-joined.

No additional configuration is required for a standard Intune-managed
device. The service starts automatically (`sc.exe query w32time` shows
`RUNNING`) and syncs on boot and periodically thereafter.

> **Reference:**
> [Windows Time Service technical reference](https://learn.microsoft.com/windows-server/networking/windows-time-service/windows-time-service-tech-ref)
> — Microsoft Learn

### 1.3 Verifying sync status

Run on the endpoint (PowerShell or cmd, no elevation required):

```powershell
w32tm /query /status
```

Expected healthy output:

```
Leap Indicator: 0(no warning)
Stratum: 3 (secondary reference - syncd by (S)NTP)
Precision: -23 (119.209ns per tick)
Root Delay: 0.0312500s
Root Dispersion: 7.7680139s
ReferenceId: 0xXXXXXXXX (source NTP server IP or hostname)
Last Successful Sync Time: 4/22/2026 3:00:00 AM
Source: time.windows.com,0x8
Poll Interval: 10 (1024s)
```

Key fields to check:

| Field | Healthy value | Notes |
|---|---|---|
| `Stratum` | 1–3 | Stratum 16 = "unsynchronised"; escalate if seen |
| `Last Successful Sync Time` | Within the last 24 h | Older than 24 h is a warning; see §1.4 |
| `Source` | Non-empty, not `Local CMOS Clock` | `Local CMOS Clock` means w32time gave up on network sync |

To see the raw offset from the reference source:

```powershell
w32tm /query /peers
```

### 1.4 Failure modes and remediation

#### Immediate resync (one-time fix)

Force an immediate resync from the current configured source:

```powershell
w32tm /resync /force
```

Then re-run `w32tm /query /status` and confirm `Last Successful Sync Time`
is within the last few seconds.

#### VM pause/restore skew

The most common source of large skew is a VM that was **paused or
saved** and then **resumed**. The guest clock freezes at the moment of
pause and can be hours behind when the VM resumes. Hypervisors
typically inject a time update on resume, but this depends on VM
integration services being installed and current.

**Remediation:**

1. Ensure VM integration services / guest additions are installed and
   up to date.
2. Force a resync immediately after resume:
   ```powershell
   w32tm /resync /force
   ```
3. If the hypervisor does not propagate time on resume, set the VM
   power-management policy to sync on restore, or configure a startup
   task that runs `w32tm /resync /force`.

For **persistent** host-clock drift (e.g. a bare-metal server whose
RTC drifts more than a few seconds per day), fix the host NTP
configuration rather than patching the guest. All guests inherit the
host's drift if integration services are performing host-to-guest time
sync.

#### w32time service stopped or disabled

```powershell
sc.exe query w32time        # check state
sc.exe start w32time        # start if stopped
sc.exe config w32time start= auto   # re-enable if disabled
```

Intune MDM policy should prevent this, but it can happen on
non-managed or partially-managed devices.

#### Clock offset registry tuning (advanced)

If the corporate environment has a time source that is consistently a
few minutes slow (unusual but possible in air-gapped environments),
the `MaxAllowedPhaseOffset` registry value controls how large an
adjustment w32time will apply in a single step vs. slewing:

```
HKLM\SYSTEM\CurrentControlSet\Services\w32time\Config\MaxAllowedPhaseOffset
```

Default: `300` seconds. Increasing this is a workaround, not a fix;
address the root-cause time source instead.

---

## 2. Network Policy

### 2.1 Outbound requirements

All traffic is **agent-initiated outbound only**. No inbound connections
are ever required or accepted.

| Destination | Port | Protocol | Purpose |
|---|---|---|---|
| `<api-server-fqdn>` (operator-configured) | 443 | TCP / HTTPS | Bundle upload, device registration, config polling |
| `*.manage.microsoft.com` + `manage.microsoft.com` | 80, 443 | TCP / HTTPS | Intune service endpoints — used by the Intune client for SCEP enrollment, MDM check-in, and (per Microsoft) for Cloud PKI service traffic. Already required for Intune itself |
| Cloud PKI CDP/OCSP host (per-tenant — extract from an issued cert) | 80 | TCP / HTTP | CRL distribution + OCSP for cert revocation checks. The exact hostname is embedded in the cert's CDP/AIA extensions and is tenant-specific; see "Verifying the actual CDP/OCSP" below |
| `time.windows.com` | 123 | UDP | NTP — used by w32time when the device is not domain-joined or domain NTP is unreachable |

> **`<api-server-fqdn>`** is set in the agent's `config.toml`
> (`[uploader] api_url = "https://..."`) and in the Intune app
> configuration policy. Operators must add their specific FQDN to the
> allowlist. There is no shared/default FQDN.

> **Cloud PKI traffic rides the standard Intune endpoints.** Per
> [Network endpoints for Microsoft Intune](https://learn.microsoft.com/intune/intune-service/fundamentals/intune-endpoints)
> — the canonical Microsoft Learn article — Cloud PKI is part of the
> Intune service surface (endpoint set 163, "Intune client and host
> service"), reached via `*.manage.microsoft.com`. There is no
> separate `*.pki.azure.net` family in Microsoft's published endpoint
> list as of April 2026; if you have an older firewall rule that
> targets `*.pki.azure.net` based on legacy guidance, replace it
> with the `*.manage.microsoft.com` rule + the cert-extracted
> CDP/OCSP host below.

> **Verifying the actual CDP/OCSP host for *your* tenant.** The CDP
> and AIA extensions in an issued cert tell you the exact hostname
> that needs reachability for revocation checks. Extract them from a
> freshly-issued device cert:
>
> ```powershell
> # Windows
> certutil -dump <path-to-cert.cer> | Select-String -Pattern "URL=http"
> ```
>
> ```bash
> # Linux / WSL with OpenSSL
> openssl x509 -text -noout -in <cert.pem> \
>   | grep -E "CRL Distribution Points|Authority Information Access" -A 5
> ```
>
> Add whatever host appears in `URL=http://...` to the firewall
> allowlist. This is the only source of truth for your tenant's
> CDP/OCSP host.

> **NTP (UDP 123):** Only required if the device is not domain-joined
> or if domain-controller NTP is unreachable. Most Intune-managed
> devices will not hit `time.windows.com` directly; include it as a
> safety net. (Also referenced in the Intune endpoints article as
> entry 165 — "Windows Autopilot — NTP Sync".)

#### Summary: FQDN allowlist for a corporate firewall

```
# api-server (operator-specific — replace with your FQDN)
<api-server-fqdn>                   TCP 443 outbound

# Microsoft Intune service (also serves Cloud PKI)
*.manage.microsoft.com              TCP 80, 443 outbound
manage.microsoft.com                TCP 80, 443 outbound

# Tenant-specific CDP/OCSP — extract from an issued cert (see note above)
<your-tenant-cdp-host>              TCP 80 outbound

# Windows Time Service (NTP fallback)
time.windows.com                    UDP 123 outbound
```

### 2.2 Inbound requirements

**None.** The agent never listens on any port. No inbound firewall rules
are required.

### 2.3 DNS requirements

Standard enterprise DNS is sufficient. The agent (and the Intune client
on the device) resolves:

- The api-server FQDN (set in config).
- `*.manage.microsoft.com` (Intune service surface; also serves Cloud PKI).
- The tenant's CDP/OCSP host as embedded in issued certs (see §2.1 for
  how to extract it via `certutil -dump`).
- `time.windows.com` (NTP fallback).

Split-DNS / split-tunnel VPN environments must ensure these names
resolve to routable addresses from the device. In particular, if the
api-server FQDN is internal-only (e.g. `pilot.corp.example.com`),
the device must be able to resolve it whether on-LAN or on VPN.

### 2.4 Firewall rule shapes

The following sections describe how to encode the outbound requirements
in the three most common policy layers.

#### 2.4.1 Windows Defender Firewall (per-device, PowerShell)

These rules allow outbound traffic from the agent process only. The
agent binary path is `C:\Program Files\CMTraceOpen\Agent\agent.exe`.

```powershell
# HTTPS to api-server
New-NetFirewallRule `
  -DisplayName "CMTraceOpen Agent — api-server HTTPS" `
  -Direction Outbound `
  -Action Allow `
  -Protocol TCP `
  -RemotePort 443 `
  -Program "C:\Program Files\CMTraceOpen\Agent\agent.exe" `
  -Profile Any

# HTTP + HTTPS to Cloud PKI CDN (CRL / OCSP)
# Note: -RemoteAddress accepts IP addresses or CIDR ranges, not FQDNs.
# Resolve the current IPs for primary-cdn.pki.azure.net and your
# <tenant-id>.pki.azure.net before running this rule, or manage via
# Intune Firewall policy which supports FQDN-based rules (see note below).
New-NetFirewallRule `
  -DisplayName "CMTraceOpen Agent — Cloud PKI CRL/OCSP" `
  -Direction Outbound `
  -Action Allow `
  -Protocol TCP `
  -RemotePort 80,443 `
  -Program "C:\Program Files\CMTraceOpen\Agent\agent.exe" `
  -Profile Any

# NTP fallback (w32time — different process).
# Intentionally NO -RemoteAddress: w32time itself handles DNS resolution
# of the configured time source (time.windows.com or domain NTP), and
# Windows Defender Firewall's -RemoteAddress accepts only IPs/CIDR, not
# FQDNs — see the note below. A generic outbound UDP 123 allow scoped to
# the w32time service host process is the correct shape.
New-NetFirewallRule `
  -DisplayName "CMTraceOpen — w32time NTP fallback" `
  -Direction Outbound `
  -Action Allow `
  -Protocol UDP `
  -RemotePort 123 `
  -Program "%SystemRoot%\System32\svchost.exe" `
  -Service "w32time" `
  -Profile Any
```

> **Note on `-RemoteAddress` with FQDNs:** The `New-NetFirewallRule`
> cmdlet's `-RemoteAddress` parameter only accepts IP addresses and CIDR
> ranges — not FQDNs. For FQDN-based rules, use Intune Endpoint security →
> Firewall → Windows Firewall rules, which supports FQDN matching on
> Windows 11 22H2+ devices. See
> [Windows Defender Firewall FQDN tagging](https://learn.microsoft.com/windows/security/threat-protection/windows-firewall/create-an-outbound-program-or-service-rule)
> for details.
>
> If you need to lock the NTP rule down to specific addresses, resolve
> `time.windows.com` and pin the resulting IPs (with the caveat that
> Microsoft uses DNS round-robin and the IPs change). For most
> environments the program/service-scoped rule above is sufficient.

**Deploy via Intune:** Endpoint security → Firewall → Create policy →
Windows Firewall rules. Encode each rule as a custom OMA-URI or use the
structured Firewall policy blade.

#### 2.4.2 Corporate / perimeter firewall

Typical next-gen firewall (Palo Alto, Fortinet, Cisco FTD, etc.) rule:

```
Source:      <device subnet or group>
Destination: <api-server-fqdn>, primary-cdn.pki.azure.net, <tenant-id>.pki.azure.net
Service:     TCP/443, TCP/80
Action:      Allow
Log:         Yes

Source:      <device subnet or group>
Destination: time.windows.com
Service:     UDP/123
Action:      Allow
Log:         Yes
```

If your firewall supports application-level FQDN categories, Azure CDN
(`*.azure.net`) is typically in a pre-built Microsoft / Azure category
that can be permitted as a group. Verify that `primary-cdn.pki.azure.net`
is included in the category before relying on it.

#### 2.4.3 Microsoft Defender for Endpoint (MDE) custom indicators

If the organisation uses MDE for endpoint network control, add custom
network indicators:

1. **Microsoft 365 Defender portal** → Settings → Endpoints →
   Indicators → Network indicators.
2. Add `Allow` indicators for:
   - `primary-cdn.pki.azure.net` (TCP 80/443)
   - `<tenant-id>.pki.azure.net` (TCP 80/443)
   - `<api-server-fqdn>` (TCP 443)
   - `time.windows.com` (UDP 123)

> **Reference:**
> [Create indicators for IPs and URLs/domains](https://learn.microsoft.com/microsoft-365/security/defender-endpoint/indicator-ip-domain)
> — Microsoft Learn

#### 2.4.4 Diagnosing blocked traffic

If you suspect a firewall is dropping traffic, capture with `netsh
trace` on the device. Wrap the capture in `try/finally` so the trace
always stops cleanly even if you Ctrl-C, the reproduction step throws,
or the PowerShell session crashes — otherwise the ETL grows
unbounded:

```powershell
# Always-stops capture pattern
$trace = "C:\Temp\agent-net.etl"
try {
    netsh trace start capture=yes tracefile=$trace maxsize=500

    # Reproduce the failure (e.g. start the agent service)
    sc.exe start CMTraceOpenAgent
    Start-Sleep -Seconds 60
}
finally {
    netsh trace stop
}

# Convert to pcapng for Wireshark (optional)
# Use Microsoft Message Analyzer or etl2pcapng:
# https://github.com/microsoft/etl2pcapng
```

For a fire-and-forget time-boxed capture (auto-stops after N seconds
even if you walk away), kick off the stop in a background job before
starting the trace:

```powershell
$trace = "C:\Temp\agent-net.etl"
$durationSec = 60
$stopJob = Start-Job -ScriptBlock {
    param($d) Start-Sleep -Seconds $d; netsh trace stop
} -ArgumentList $durationSec
netsh trace start capture=yes tracefile=$trace maxsize=500
sc.exe start CMTraceOpenAgent
Wait-Job $stopJob | Receive-Job
```

Look for RST packets, ICMP port-unreachable, or TCP SYN with no SYN-ACK
on the relevant destination IP/port. A successful TLS handshake will
show a TCP three-way handshake followed by TLS `ClientHello` /
`ServerHello`.

---

## 3. Cross-references

- [`docs/wave4/03-beta-pilot-runbook.md` — Appendix A.2 (Network can't reach api-server)](./03-beta-pilot-runbook.md#a2--network-cant-reach-api-server)
- [`docs/wave4/03-beta-pilot-runbook.md` — Appendix A.3 (Time skew)](./03-beta-pilot-runbook.md#a3--time-skew)
- [`docs/provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md) — VM provisioning runbook (includes VM-specific clock-skew notes)
- [`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md) — Cloud PKI setup; the PKI CDN URLs in §2.1 correspond to the CRL/OCSP CDPs embedded in the cert this profile issues
- [Windows Time Service technical reference](https://learn.microsoft.com/windows-server/networking/windows-time-service/windows-time-service-tech-ref) — Microsoft Learn
- [w32tm command reference](https://learn.microsoft.com/windows-server/networking/windows-time-service/windows-time-service-tools-and-settings) — Microsoft Learn
- [Cloud PKI for Microsoft Intune — overview](https://learn.microsoft.com/intune/cloud-pki/) — Microsoft Learn
- [Network endpoints for Microsoft Intune](https://learn.microsoft.com/intune/intune-service/fundamentals/intune-endpoints) — Microsoft Learn (the canonical source for Cloud PKI's network endpoints — see endpoint set 163, "Intune client and host service")
- [Microsoft Intune cloud PKI fundamentals](https://learn.microsoft.com/intune/cloud-pki/fundamentals) — Microsoft Learn (chain validation, CDP/AIA mechanics)
- [Create indicators for IPs and URLs/domains (MDE)](https://learn.microsoft.com/microsoft-365/security/defender-endpoint/indicator-ip-domain) — Microsoft Learn
- [Windows Defender Firewall — create an outbound rule](https://learn.microsoft.com/windows/security/threat-protection/windows-firewall/create-an-outbound-program-or-service-rule) — Microsoft Learn
