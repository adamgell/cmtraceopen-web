# 03 — Intune + Cloud PKI Onboarding (mTLS Client Cert Issuance)

Runbook for standing up Microsoft Intune Cloud PKI so the cmtraceopen agent running
on an Entra-joined Windows device can obtain a client certificate used for mTLS
authentication to the api-server. This is a **Wave 3** prerequisite: when the
mTLS termination code lands in api-server, it will validate client certs against
a trust bundle sourced from this runbook's exported issuing CA.

---

## Purpose

The cmtraceopen agent authenticates to the api-server with a client certificate
issued by Intune Cloud PKI. Each Entra-joined test device receives a cert via an
Intune certificate profile.

- **SAN URI**: `device://{tenant-id}/{aad-device-id}` — api-server parses this
  URI and maps it to the internal `device_id`. This mapping is load-bearing and
  should not be changed without also changing the api-server SAN parser.
- **EKU**: `clientAuth` (OID `1.3.6.1.5.5.7.3.2`) only.
- **Lifetime**: 365 days, auto-renewed at 80 %.
- **Key storage**: LocalMachine\My, non-exportable.

When Wave 3 lands, the api-server trust bundle (the exported issuing-CA PEM
produced in **Step 6**) is what api-server's `client_ca_bundle` will point at.

---

## Licensing — read before continuing

**Intune Cloud PKI is not free.** It is sold either as part of the **Intune
Suite** add-on or as a **Cloud PKI standalone** SKU. Confirm your tenant has an
active subscription — or start a trial — **before** following this runbook.

Approximate list pricing at authorship (verify against current Microsoft pricing
before purchase — these numbers move):

| SKU                         | Approx. list price per user/month |
| --------------------------- | --------------------------------- |
| Intune Suite (bundle)       | ~ $10                             |
| Cloud PKI standalone add-on | ~ $2                              |

Authoritative docs and pricing:

- Cloud PKI overview: <https://learn.microsoft.com/mem/intune/protect/microsoft-cloud-pki-overview>
- Intune Suite: <https://learn.microsoft.com/mem/intune/fundamentals/intune-add-ons>
- Pricing page (subject to change): <https://www.microsoft.com/security/business/microsoft-intune-pricing>

### Fallbacks if Cloud PKI is unavailable

If your tenant cannot acquire Cloud PKI, two operationally heavier options exist:

1. **Intune SCEP via NDES** — requires hosting NDES + an AD CS CA, Azure App
   Proxy / reverse proxy, and the Intune Certificate Connector. Operationally
   painful and a common source of outages. Not recommended for MVP.
2. **Intune PKCS Connector against on-prem AD CS** — requires existing AD CS
   infrastructure. Simpler than SCEP/NDES but still requires on-prem dependencies.

Either fallback will be documented in a future `07-fallback-pki.md` runbook and
is **not required for MVP**.

---

## Prerequisites

- Entra (Azure AD) tenant with **Global Administrator** access.
- Intune license — **Plan 1** or **Plan 2**, or a bundle that includes Intune
  (M365 E3/E5, M365 Business Premium, EMS E3/E5).
- **Intune Suite** add-on or **Cloud PKI standalone** SKU assigned to the
  tenant.
- At least one Entra-joined Windows 10/11 test device (see
  [`01-entra-test-device.md`](./01-entra-test-device.md)).
- `openssl` available locally for validating the exported CA PEM.

> Some of the steps below require a live tenant + admin session and cannot be
> automated. Where a portal click is required it is called out explicitly.

---

## Step 1 — Enable Intune in the tenant (skip if already enabled)

1. Portal: <https://intune.microsoft.com>.
2. Sign in as Global Admin.
3. Confirm the left-nav shows **Devices**, **Apps**, and **Endpoint security**.
4. If the portal prompts you to set the MDM authority, accept the default
   (Microsoft Intune). Full walkthrough:
   <https://learn.microsoft.com/mem/intune/fundamentals/free-trial-sign-up>.

---

## Step 2 — Create a device group for test devices

1. Entra portal → <https://entra.microsoft.com> → **Groups** → **New group**.
2. Group type: **Security**.
3. Name: `cmtraceopen-testdevices`.
4. Membership type: **Assigned** for MVP. For scale, switch to **Dynamic Device**
   with rule:

   ```kusto
   (device.displayName -startsWith "cmtraceopen-testvm")
   ```

5. Add the test VM from [`01-entra-test-device.md`](./01-entra-test-device.md)
   as a member.

Docs: <https://learn.microsoft.com/entra/identity/users/groups-create-rule>.

---

## Step 3 — Provision Intune Cloud PKI

1. Intune admin center → **Tenant administration** → **Connectors and tokens**
   → **Cloud PKI**.
2. Click **Set up Cloud PKI** (or **Create** if already provisioned).
3. Choose the **root CA model**:
   - **Microsoft-managed root** (default). Simpler — Microsoft hosts and rotates
     the root. Recommended for greenfield.
   - **Bring Your Own Root (BYOR)**. Cloud PKI issues from an intermediate
     chained to your enterprise root. Recommended when the org already has PKI
     governance.
4. Provision an **issuing CA** under the chosen root:
   - CA name: `cmtraceopen-issuing-ca` (placeholder — match your naming).
   - Validity: accept default unless policy dictates otherwise.
   - Key algorithm: **ECDSA P-256** if available (smaller handshakes); otherwise
     RSA 2048.
5. After provisioning, download the **issuing CA certificate (Base64 PEM)**:
   - Portal path: Cloud PKI → select the issuing CA → **Download CA certificate**.
   - Save locally to `./config/ca/cloud-pki-issuing.pem` in the deploying repo.
   - Ensure `./config/ca/` is gitignored. Document the expected path in the
     repo README.

> The Base64 PEM from this step is the **trust anchor** that feeds the
> api-server's `client_ca_bundle` in Step 6.

Docs: <https://learn.microsoft.com/mem/intune/protect/cloud-pki-create-ca>.

---

## Step 4 — Create the client certificate profile

1. Intune → **Devices** → **Configuration** → **Create** → **New policy**.
2. Platform: **Windows 10 and later**.
3. Profile type: **Templates** → **PKCS certificate**.
   (Cloud PKI supports PKCS for Windows. Use SCEP only if you specifically need
   the Cloud PKI SCEP endpoint — PKCS is simpler here.)
4. Name: `cmtraceopen-client-cert`.
5. Settings:
   - **Certificate type**: **Device**.
   - **Certificate store**: **Machine**.
   - **Key usage**: **Digital Signature** + **Key Encipherment**.
   - **Extended key usage (EKU)**: **Client Authentication**
     (`1.3.6.1.5.5.7.3.2`) — this EKU **only**; no Server Authentication.
   - **Key algorithm**: **ECDSA P-256** (preferred) or **RSA 2048** if ECDSA is
     not available on your Cloud PKI issuing CA.
   - **Subject name format**: `CN={{DeviceId}}` — display only; not parsed by
     the api-server.
   - **Subject alternative name (SAN)**:
     - Type: **URI**
     - Value: `device://{{TenantId}}/{{DeviceId}}`

     > **Load-bearing**: api-server reads this SAN URI and derives `device_id`
     > from `{aad-device-id}`. Any change here must be coordinated with the
     > api-server SAN parser.
   - **Validity period**: **365 days**.
   - **Renewal threshold**: **20 %** (renews at 80 % of lifetime — Intune
     default).
   - **Key storage provider (KSP)**: **Enroll to Trusted Platform Module (TPM)
     KSP, otherwise fail**. Ensures private key is TPM-bound where possible.
6. **Certification authority**: select the Cloud PKI issuing CA created in
   Step 3.
7. **Assignments**: include the `cmtraceopen-testdevices` group from Step 2.
8. Review + create.

Docs: <https://learn.microsoft.com/mem/intune/protect/certificates-pfx-configure>.

### Intune PKCS variable reference

| Placeholder       | Resolves to                                  |
| ----------------- | -------------------------------------------- |
| `{{DeviceId}}`    | Entra/AAD device ID GUID                     |
| `{{TenantId}}`    | Entra tenant ID GUID                         |
| `{{AAD_Device_ID}}` | Alias for `DeviceId` in some profile types |

Reference: <https://learn.microsoft.com/mem/intune/configuration/custom-settings-windows-10>.

---

## Step 5 — Verify cert issuance on the test device

1. On the test VM, force a policy refresh:

   ```powershell
   dsregcmd /refreshprt
   Start-Process -FilePath "$env:windir\system32\DeviceEnroller.exe" -ArgumentList "/o","$env:USERDOMAIN","/c","/b"
   # Or in Settings → Accounts → Access work or school → Info → Sync.
   ```

2. Wait 15–30 minutes for the PKCS profile to apply on first assignment.
3. Open the LocalMachine certificate store:

   ```powershell
   certlm.msc
   # Navigate: Personal → Certificates.
   ```

4. Look for a certificate whose:
   - **Subject** is `CN=<aad-device-id>`.
   - **Issuer** is the Cloud PKI issuing CA from Step 3.
   - **Enhanced Key Usage** is `Client Authentication` only.

5. Verify the SAN URI from PowerShell:

   ```powershell
   Get-ChildItem Cert:\LocalMachine\My |
     Where-Object { $_.Issuer -like "*cmtraceopen-issuing-ca*" } |
     ForEach-Object {
       $san = $_.Extensions |
         Where-Object { $_.Oid.Value -eq '2.5.29.17' } |
         ForEach-Object { $_.Format($true) }
       [pscustomobject]@{
         Thumbprint = $_.Thumbprint
         Subject    = $_.Subject
         NotAfter   = $_.NotAfter
         SAN        = $san
       }
     } | Format-List
   ```

   Expected SAN line:

   ```
   URL=device://<tenant-id>/<aad-device-id>
   ```

6. Confirm the private key is **non-exportable**:

   ```powershell
   $cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.Subject -match 'CN=<aad-device-id>' }
   $cert.PrivateKey.CspKeyContainerInfo.Exportable  # should be False
   # (On CNG keys, inspect via: certutil -store My <thumbprint>.)
   ```

   Also confirm **Key archival** is **off** on the cert detail view
   (`certlm.msc` → double-click cert → **Details** → **Archived Key** should be
   absent / No).

---

## Step 6 — Export the issuing CA for the api-server trust bundle

1. You already have the Base64 PEM from Step 3 (`cloud-pki-issuing.pem`).
2. Validate it:

   ```bash
   openssl x509 -in cloud-pki-issuing.pem -noout -text
   ```

   Confirm:
   - **Subject** matches the issuing-CA name from Step 3.
   - **Basic Constraints**: `CA:TRUE`.
   - **Key Usage**: `Certificate Sign, CRL Sign` (plus any additions your
     governance requires).
   - EKU is either absent (unrestricted CA — expected) or includes
     `Client Authentication`.
3. Verify the fingerprint against what the Intune portal shows for the issuing
   CA:

   ```bash
   openssl x509 -in cloud-pki-issuing.pem -noout -fingerprint -sha256
   ```

4. Deploy to the api-server host:
   - **Prod**: `/etc/cmtraceopen/certs/client-ca.pem`, owned by the api-server
     service user, mode `0644`.
   - **Dev**: `./config/ca/cloud-pki-issuing.pem` relative to the compose root.
5. `docker-compose.yml` mount (Wave 3 — reference; uses the
   `CMTRACE_*` prefix the api-server actually reads):

   ```yaml
   services:
     api-server:
       volumes:
         - ./config/ca/gell-pki-root.pem:/etc/cmtraceopen/certs/client-ca.pem:ro
         - ./config/tls/server.crt:/etc/cmtraceopen/certs/server.crt:ro
         - ./config/tls/server.key:/etc/cmtraceopen/certs/server.key:ro
       environment:
         CMTRACE_TLS_ENABLED: "true"
         CMTRACE_TLS_CERT: /etc/cmtraceopen/certs/server.crt
         CMTRACE_TLS_KEY: /etc/cmtraceopen/certs/server.key
         CMTRACE_CLIENT_CA_BUNDLE: /etc/cmtraceopen/certs/client-ca.pem
         CMTRACE_MTLS_REQUIRE_INGEST: "true"
         CMTRACE_SAN_URI_SCHEME: "device"
   ```

### api-server config contract (for the Wave 3 mTLS agent)

> **Note:** the env-var prefix shipped in code is `CMTRACE_*` (not the
> `CMTRACEOPEN_*` placeholder originally drafted in this runbook). The
> rest of the platform — `CMTRACE_LISTEN_ADDR`, `CMTRACE_AUTH_MODE`, etc.
> — already uses `CMTRACE_`, so the mTLS surface follows suit.
> CRL polling (`CMTRACE_CRL_*`) is a follow-up; only the `CMTRACE_TLS_*`
> + `CMTRACE_MTLS_*` + `CMTRACE_SAN_URI_SCHEME` + `CMTRACE_CLIENT_CA_BUNDLE`
> rows below are wired in the Wave 3 PR.

| Env var                       | Meaning                                                               | Example                                       |
| ----------------------------- | --------------------------------------------------------------------- | --------------------------------------------- |
| `CMTRACE_TLS_ENABLED`         | Master switch — turns on TLS termination + the mTLS surface           | `true`                                        |
| `CMTRACE_TLS_CERT`            | Absolute path to the server's own PEM-encoded cert chain              | `/etc/cmtraceopen/certs/server.crt`           |
| `CMTRACE_TLS_KEY`             | Absolute path to the server's PEM-encoded private key                 | `/etc/cmtraceopen/certs/server.key`           |
| `CMTRACE_CLIENT_CA_BUNDLE`    | Absolute path to the PEM-encoded Cloud PKI trust bundle               | `/etc/cmtraceopen/certs/client-ca.pem`        |
| `CMTRACE_MTLS_REQUIRE_INGEST` | If `true`, ingest 401s without a verified client cert (default: true) | `true`                                        |
| `CMTRACE_SAN_URI_SCHEME`      | Expected URI scheme in the client cert SAN (default: `device`)        | `device`                                      |
| `CMTRACE_CRL_URL`             | _Future:_ CRL distribution point polled hourly                        | `https://<cloud-pki-crl-endpoint>/cmtrace.crl` |
| `CMTRACE_CRL_REFRESH_SECS`    | _Future:_ seconds between CRL refreshes (default 3600)                | `3600`                                        |

Filesystem convention:

- `./config/ca/cloud-pki-issuing.pem` — dev trust bundle (gitignored).
- `/etc/cmtraceopen/certs/client-ca.pem` — prod trust bundle path.

---

## Renewal and revocation

- **Auto-renewal**: Intune triggers PKCS renewal at 80 % of lifetime (~292 days
  in, for a 365-day cert). No manual action needed on devices that remain
  enrolled and compliant.
- **Revocation**: Intune admin center → **Devices** → **Configuration** →
  select the PKCS profile → **Per-device status** → select device → **Revoke**.
  Also available via the Cloud PKI certificate list: Cloud PKI → issuing CA →
  **Certificates** → select → **Revoke**.
- **api-server CRL refresh**: the Wave 3 mTLS agent polls
  `CMTRACEOPEN_CRL_URL` hourly (`CMTRACEOPEN_CRL_REFRESH_SECS=3600`) and rejects
  any client cert whose serial appears in the CRL. See the Wave 3 section of
  the project plan.

Docs: <https://learn.microsoft.com/mem/intune/protect/cloud-pki-revoke-cert>.

---

## "Done" criteria

- [ ] Test VM enrolled in the `cmtraceopen-testdevices` group.
- [ ] Cloud PKI issuing CA provisioned and PEM downloaded to
      `./config/ca/cloud-pki-issuing.pem`.
- [ ] PKCS certificate profile created and assigned.
- [ ] Cert visible in `Cert:\LocalMachine\My` on the test VM with:
      - Subject `CN=<aad-device-id>`
      - Issuer matching the Cloud PKI issuing CA
      - SAN URI `device://<tenant-id>/<aad-device-id>`
      - EKU `clientAuth` only
      - Private key non-exportable
- [ ] `openssl x509 -in ... -noout -text` validates the CA PEM.
- [ ] Trust bundle staged at `/etc/cmtraceopen/certs/client-ca.pem` (prod) and
      `./config/ca/cloud-pki-issuing.pem` (dev).
- [ ] Env-var contract above matches what the api-server Wave 3 mTLS agent
      will read.

---

## References

- Cloud PKI overview: <https://learn.microsoft.com/mem/intune/protect/microsoft-cloud-pki-overview>
- Create a Cloud PKI CA: <https://learn.microsoft.com/mem/intune/protect/cloud-pki-create-ca>
- PKCS certificate profiles: <https://learn.microsoft.com/mem/intune/protect/certificates-pfx-configure>
- Intune Suite / add-ons: <https://learn.microsoft.com/mem/intune/fundamentals/intune-add-ons>
- Revoke a Cloud PKI cert: <https://learn.microsoft.com/mem/intune/protect/cloud-pki-revoke-cert>
- Dynamic device groups: <https://learn.microsoft.com/entra/identity/users/groups-create-rule>
