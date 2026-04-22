# Wave 4 — Code Signing Strategy

**Status:** Design (not wired up). Tracks the strategy for signing the
`cmtraceopen-agent` Windows binaries before they ship to managed
devices via Intune. Companion stub workflow lives at
`.github/workflows/sign-agent.yml`.

---

## 1. Why sign

Three concrete failures we hit if we ship unsigned binaries:

1. **SmartScreen on first run.** Windows Defender SmartScreen flags
   any unsigned EXE downloaded from the internet with a "Windows
   protected your PC" prompt. Users have to click *More info → Run
   anyway* — a non-starter for a log-collection agent intended to run
   silently.
2. **Intune Win32 deployments fail at scale.** Intune does not block
   unsigned MSIs outright, but enterprise-managed devices with
   AppLocker / Smart App Control / WDAC policies will refuse to
   execute them. Devices report "untrusted publisher" and the
   deployment shows as failed in the Intune portal even though the
   MSI ran.
3. **GitHub-supply-chain attack.** An attacker who breaches the
   `adamgell/cmtraceopen-web` GitHub repo (compromised PAT, malicious
   dependency, leaked OIDC trust) can swap the published artifact.
   Without signing, downstream consumers have no way to detect the
   substitution. Signing binds every released binary to a key that
   never leaves Microsoft's HSM.

Signing closes all three gaps with one operation.

## 2. What needs signing

| Artifact                         | Signing tech                       | Wave   |
| -------------------------------- | ---------------------------------- | ------ |
| `cmtraceopen-agent.exe`          | Windows Authenticode (ATS cert)    | Wave 4 |
| `CMTraceOpenAgent.msi`           | Windows Authenticode (ATS cert)    | Wave 4 |
| `ghcr.io/adamgell/cmtraceopen-api` | Sigstore cosign (keyless / OIDC) | Wave 5+ (deferred) |

The agent EXE and its MSI installer use the same Authenticode cert.
The api-server container image is a separate concern (see §9).

## 3. Recommended cert path: Azure Trusted Signing (ATS)

Azure Trusted Signing (ATS, formerly Azure Code Signing) is the
recommended path. Comparison vs the traditional EV cert route:

| Concern                 | Azure Trusted Signing                              | GlobalSign / DigiCert EV                             |
| ----------------------- | -------------------------------------------------- | ---------------------------------------------------- |
| Cost model              | Pay-per-signature (~$15/mo base + per-sig fees)    | $400+/yr per cert + HSM token shipping               |
| Key custody             | Microsoft-managed HSM, key never exfiltrated       | EV token (USB HSM) — physical custody, lose-it risk  |
| GitHub Actions support  | First-party action (`Azure/trusted-signing-action`) | Manual: stage token on self-hosted runner or KSP    |
| Cert rotation           | Automatic — short-lived certs minted per signature | Manual; new cert every 1–3 years                     |
| Identity binding        | Tied to Entra tenant — only authorized SPs sign   | Whoever physically holds the token can sign         |
| SmartScreen reputation  | Inherits Microsoft Identity Verified PCA trust    | Builds reputation per-cert (slow ramp)              |

**Caveat — company age requirement.** ATS Identity Verification
requires the legal entity to be **at least 18 months old** (Microsoft
verifies via Dun & Bradstreet). If the legal entity behind
cmtraceopen is younger than that, ATS rejects the verification and we
fall back to GlobalSign Identity Validation (3–7 day verification) or
an EV cert via SSL.com / DigiCert. The workflow shape below stays the
same — only the signing action swaps. See §8.

> **Open question (flagged, not blocking design):** is the legal
> entity behind `adamgell/cmtraceopen-web` ≥ 18 months old? See §10.

## 4. ATS setup (one-time)

1. **Create the ATS resource.** Azure portal → *Create a resource* →
   search "Trusted Signing". Pick a region close to the GitHub
   Actions runner (East US 2 is a safe default).
   Docs: [Azure Trusted Signing — quickstart](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart).

2. **Create a Code Signing Account + Certificate Profile.** Inside
   the ATS resource:
    - *Code Signing Accounts* → *Create*. Name: `cmtraceopen-signing`.
    - *Certificate Profiles* → *Create*. Type: **Public Trust**.
      Profile name: `cmtraceopen-agent`. Identity Verification:
      attach the org's verified identity (next step).
   Docs: [Set up an Azure Trusted Signing account and certificate profile](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart-trusted-signing-account-certificate-profile).

3. **Pass Identity Verification.** ATS calls Microsoft's verification
   service which cross-checks the org's legal name against
   Dun & Bradstreet. Provide the legal entity name, registered
   address, and a verifying admin contact. Provisioning takes
   **3–7 business days** in normal cases — plan around this.
   Docs: [Identity Validation](https://learn.microsoft.com/en-us/azure/trusted-signing/concept-trusted-signing-resources-roles-renewal).

4. **Wire up GitHub Actions auth (federated OIDC, no secrets).**
    - Create an Entra app registration: *App registrations* → *New*.
      Name: `gh-actions-cmtraceopen-signer`.
    - Grant it the **Trusted Signing Certificate Profile Signer**
      role at the cert profile's resource scope (NOT subscription
      scope — least privilege):
      *ATS resource → IAM → Add role assignment → Trusted Signing
      Certificate Profile Signer → assign to the SP*.
    - Add a **Federated credential** to the app:
        - Issuer: `https://token.actions.githubusercontent.com`
        - Subject: `repo:adamgell/cmtraceopen-web:ref:refs/tags/agent-v*`
        - Audience: `api://AzureADTokenExchange`
      This restricts signing to runs triggered by `agent-v*` tags
      only — PRs and `main` pushes physically cannot sign.
   Docs: [Configure GitHub Actions OIDC for Azure](https://learn.microsoft.com/en-us/azure/developer/github/connect-from-azure-openid-connect).

5. **Document the four GitHub repository secrets/variables.**
   No client secret — federated creds only.

   | Name                                | Where    | Source                                  |
   | ----------------------------------- | -------- | --------------------------------------- |
   | `AZURE_TENANT_ID`                   | Variable | Entra → Overview → Tenant ID            |
   | `AZURE_CLIENT_ID`                   | Variable | App registration → Application (client) ID |
   | `AZURE_TRUSTED_SIGNING_ENDPOINT`    | Variable | ATS resource → Overview → Endpoint URL  |
   | `AZURE_TRUSTED_SIGNING_ACCOUNT_NAME` | Variable | ATS resource → Code Signing Account name |

   These are non-sensitive (no creds), so use repository **variables**
   rather than secrets — easier to audit in PR diffs.

## 5. Reusable signing workflow

The stub at `.github/workflows/sign-agent.yml` is a `workflow_call`
reusable workflow. The future `agent-msi.yml` will:

1. Build the unsigned `.msi` (cargo build → WiX harvest → light/candle).
2. Upload it as an artifact.
3. Call `sign-agent.yml` with `artifact-name: CMTraceOpenAgent.msi`
   and a human-readable `description`.

Inside `sign-agent.yml`:

| Step                                | Why                                              |
| ----------------------------------- | ------------------------------------------------ |
| `actions/download-artifact@v4`      | Pull the unsigned MSI from the calling workflow  |
| `azure/login@v2` (federated OIDC)   | No client secret. Requires `id-token: write`     |
| `azure/trusted-signing-action@v0.5.0` (pinned) | Sign the MSI in place                |
| Verify (`signtool verify /pa /v`)   | Fail the run if the signature doesn't validate   |
| `actions/upload-artifact@v4`        | Re-upload the signed MSI for the calling workflow |

Required job permissions:

```yaml
permissions:
  id-token: write   # OIDC federation to Azure
  contents: read    # checkout / artifact download
```

## 6. Verification

After signing, `signtool verify` should produce a chain ending at the
ATS root (currently **Microsoft ID Verified Code Signing PCA 2024** —
verify against the [ATS root CA reference](https://learn.microsoft.com/en-us/azure/trusted-signing/concept-trusted-signing-cert-management)
each release in case Microsoft rotates).

```powershell
signtool verify /pa /v /all CMTraceOpenAgent.msi
```

Expected output (abbreviated):

```
Signing Certificate Chain:
  Issued to: Microsoft Identity Verification Root Certificate Authority 2020
    Issued to: Microsoft ID Verified Code Signing PCA 2024
      Issued to: <Your Org Legal Name>
File is signed and timestamped.
Successfully verified: CMTraceOpenAgent.msi
```

Add a `verify-signature` step to `sign-agent.yml` so the run fails
loudly if the cert chain ever breaks. Cheap insurance.

## 7. Signing cadence

ATS bills per signature. PR validation MUST NOT sign. Cadence:

| Trigger                        | Signs?  | Why                                                        |
| ------------------------------ | ------- | ---------------------------------------------------------- |
| Push to `main`                 | No      | Validates build/test only. No release artifact produced.   |
| Pull request                   | No      | Forks have no OIDC trust. ATS bill protection.             |
| Tag `agent-v*` (release)       | **Yes** | Only released artifacts get signatures. Federated cred SUB matches this ref pattern only. |
| `workflow_dispatch` (manual)   | No (default) | Reserve for ad-hoc dry-runs. If signing is desired, opt in via input. |

The federated credential's `subject` claim
(`refs/tags/agent-v*`) physically prevents non-tag runs from
acquiring an Azure token, so the cadence is enforced at the auth
layer, not just by branch logic. Defense in depth.

## 8. Backup plan if ATS unavailable

If Identity Verification fails (company age, D&B mismatch, etc.):

- **GlobalSign Identity Validation** code-signing cert. 3–7 day
  verification. ~$300/yr. Cert lives in Azure Key Vault; sign via
  `AzureSignTool` invoked from the same workflow shape as ATS.
- **DigiCert KeyLocker.** Cloud-hosted HSM, similar to ATS but
  vendor-locked to DigiCert. Has a first-party GitHub Action.
- **SSL.com EV.** Cheapest EV option but requires shipping a USB
  HSM token to a self-hosted runner — operational burden we'd rather
  avoid.

In all three cases the workflow shape (download → sign → verify →
upload) is unchanged; only the *sign* step swaps. The stub workflow
isolates the sign action so the swap is a one-step change.

## 9. Sigstore cosign for api-server image (deferred)

The api-server container image (`ghcr.io/adamgell/cmtraceopen-api`)
is signed differently — Authenticode is for PE binaries, not OCI
artifacts. When `publish-api.yml` reaches prod-grade, add:

- `sigstore/cosign-installer@v3` to the publish workflow.
- A `cosign sign --yes <image-digest>` step running with
  `id-token: write` permission so cosign can do **keyless signing**
  via GitHub OIDC — the signature chains to the GitHub Actions
  identity (the workflow's repo + ref), no long-lived keys to manage.
- A `cosign verify` step in the BigMac deploy path (Ansible
  `compose_stack` role) so a poisoned image fails the deploy instead
  of starting up.

Tracking ticket: open after Wave 5 once the GHCR pipeline stabilizes.
Listed here so reviewers know it's intentionally out-of-scope for
Wave 4.

## 10. Open questions

- [ ] **Company age.** Is the legal entity behind `adamgell/cmtraceopen-web`
  ≥ 18 months old (D&B-verifiable)? If no, switch to §8 backup
  before requesting Identity Verification.
- [ ] **Existing org cert?** Is there a code-signing cert already
  provisioned in an Azure Key Vault / org HSM somewhere we should
  reuse instead of standing up ATS fresh?
- [ ] **Legal entity for IV.** Which legal name + registered address
  goes on the Identity Verification submission? (Cert *Subject* /
  *O=* field is locked to this for the cert's lifetime.)
- [ ] **Signing region.** ATS region pick — East US 2 default OK or
  do we want a region closer to the GitHub-hosted runner pool to
  shave seconds off the per-signature round trip?

---

**Next step:** answer the four questions above, then start the
Identity Verification clock in parallel with implementing
`agent-msi.yml`. The 3–7 day IV wait is the critical-path item.
