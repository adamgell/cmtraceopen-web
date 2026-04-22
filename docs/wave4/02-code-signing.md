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
   substitution. Signing binds every released binary to a key whose
   private material never leaves the Cloud PKI HSM.

Signing closes all three gaps with one operation.

## 2. What needs signing

| Artifact                         | Signing tech                          | Wave   |
| -------------------------------- | ------------------------------------- | ------ |
| `cmtraceopen-agent.exe`          | Windows Authenticode (Cloud PKI cert) | Wave 4 |
| `CMTraceOpenAgent.msi`           | Windows Authenticode (Cloud PKI cert) | Wave 4 |
| `ghcr.io/adamgell/cmtraceopen-api` | Sigstore cosign (keyless / OIDC)    | Wave 5+ (deferred) |

The agent EXE and its MSI installer use the same Authenticode cert.
The api-server container image is a separate concern (see §10).

## 3. Recommended cert path: Intune Cloud PKI + self-hosted runner

Intune Cloud PKI is the right call for this project. Reasoning, in
descending order of importance:

- **The Cloud PKI hierarchy is already deployed.** Per
  `docs/provisioning/03-intune-cloud-pki.md` and
  `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`, the Gell
  PKI Root + Issuing CAs are live in the tenant. The Issuing CA
  already lists `codeSigning` (1.3.6.1.5.5.7.3.3) in its EKU set, so
  it can mint a leaf with code-signing usage on demand.
- **The root chain is already trusted in `LocalMachine\Root` on every
  Intune-managed device.** Confirmed by the operator: Cloud PKI's
  trusted-root distribution rides the same MDM channel that pushes
  client-auth certs to the agent fleet. A Cloud-PKI-signed binary
  therefore passes SmartScreen and Intune Win32 publisher trust on
  every device in the cmtraceopen pilot fleet **without any extra
  cert distribution work**.
- **No Identity Verification wait.** Azure Trusted Signing requires a
  3–7 day Microsoft Identity Verification. Cloud PKI is
  tenant-internal — no third-party verification, no D&B lookup,
  setup measured in hours not days.
- **No per-signature cost.** ATS bills per signature; Cloud PKI is
  flat. Costs in §7.
- **Cloud PKI keys are HSM-backed and non-exportable.** Same security
  property as ATS (different HSM, same outcome). Caveat: because the
  key is non-exportable, the cert cannot be moved into Azure Key
  Vault and signed via `AzureSignTool`. It must be used in-place on
  the machine that holds it. That shapes the build topology — see
  next bullet.
- **The shape: build VM + self-hosted runner + one-member group cert
  profile + signtool from cert store.** Stand up a single Windows VM
  as a GitHub Actions self-hosted runner. Enroll it in Intune. Push
  it a Cloud PKI cert profile with `codeSigning` EKU, scoped via a
  one-member Entra group containing only that VM. CI signs by calling
  `signtool sign /a` on the runner — `signtool` finds the cert by EKU
  match in `LocalMachine\My`. No Azure auth, no OIDC, no secrets to
  manage.

### Tradeoff table

| Concern                | Intune Cloud PKI (recommended)                            | Azure Trusted Signing                                | DigiCert / GlobalSign EV                            |
| ---------------------- | --------------------------------------------------------- | ---------------------------------------------------- | --------------------------------------------------- |
| Procurement            | None — already deployed in this tenant                    | Microsoft Partner Center signup + IV                 | CA contract + token shipping                        |
| Setup time             | ~1 day (cert profile + VM + runner)                       | 3–7 business days (Identity Verification)            | 1–7 days (CA verification + token in mail)          |
| Cost model             | $0 (Cloud PKI is included in Intune Suite)                | ~$15/mo base + per-signature fees                    | $300–700/yr per cert + HSM token                    |
| Key custody            | Cloud PKI HSM, non-exportable                             | Microsoft-managed HSM, key never exfiltrated         | EV USB token, physical custody risk                 |
| GitHub Actions auth    | None — cert is local to the self-hosted runner            | Federated OIDC, no client secret                     | Manual: stage token on self-hosted runner or KSP    |
| Cert rotation          | Automatic via Intune cert-profile renewal                 | Automatic — short-lived per-signature certs          | Manual; new cert every 1–3 years                    |
| Trust scope            | Intune-managed devices in this tenant only                | Public — chains to Microsoft ID Verified PCA         | Public — chains to commercial CA                    |
| SmartScreen / Win32 trust on managed devices | Pass (root in `LocalMachine\Root` via MDM) | Pass (public PCA)                                    | Pass (public CA reputation)                         |
| Trust on a fresh box outside Intune | **No** (root not present)                        | Yes                                                  | Yes                                                 |

For this project — Wave 4 pilot deploys to Intune-managed devices
only — Cloud PKI wins on every concern except "trust outside the
managed fleet," which is not a Wave 4 requirement. ATS is preserved
as the future broader-release path; see §9.

## 4. Setup steps for Cloud PKI signing

One-time bootstrap. Each step is on the order of minutes;
end-to-end is a half day plus Intune sync wait.

1. **Create the code-signing cert profile in Intune.**
    - Intune admin center → *Devices → Configuration → Create*.
    - Profile type: **PKCS certificate** (Cloud PKI). Platform: Windows 10/11.
    - Name: `cmtraceopen-codesign-builder`.
    - Certification authority: **Gell - PKI Issuing** (the issuing
      CA already lists `codeSigning` in its EKU set per
      `reference_cloud_pki.md`).
    - Key storage provider (KSP): Microsoft Software Key Storage
      Provider (or Platform if TPM-backed is desired on the VM).
    - Key algorithm: RSA-2048 (or ECDSA-P256 if your build chain
      tolerates it; RSA-2048 is the safe default for `signtool`).
    - Enhanced key usage: **Code Signing (1.3.6.1.5.5.7.3.3)** —
      *only* this EKU; do not also tick Client Authentication (we
      want a strict EKU so signtool can't accidentally pick a
      device-auth cert).
    - Subject name format: `CN={{DeviceName}}-codesign` is fine; the
      subject is cosmetic for code signing (verifiers care about the
      EKU + chain, not the leaf CN).
    - Certificate validity period: **1 year** (matches the cadence
      of the device-auth profile in `03-intune-cloud-pki.md`).
    - Renewal threshold: 20% (Intune auto-renews when 20% of validity
      remains).

2. **Create a one-member Entra group.**
    - Entra admin center → *Groups → New group*.
    - Type: Security. Name: `cmtraceopen-build-machines`.
    - Membership type: Assigned (not dynamic — we want explicit control).
    - Owner: the operator account.
    - Leave members empty for now; the build VM joins after enrollment.

3. **Assign the cert profile to the group.**
    - Back in the cert profile from step 1 → *Assignments → Included groups*.
    - Add `cmtraceopen-build-machines`. **Do not assign to All Devices**
      — the code-signing cert must land on the build VM only,
      otherwise every managed device becomes a potential signer.

4. **Stand up the build VM.**
    - Windows 11 Pro/Enterprise or Server 2022. Operator's choice of
      hypervisor: Hyper-V on a separate Windows host, an Azure VM
      (B2s ≈ $30/mo, lowest viable spec), or on-prem Equinix metal.
      BigMac26 is macOS — Hyper-V on it isn't an option. See §11.
    - 2 vCPU / 4 GB RAM / 64 GB disk is enough for `cargo build` +
      WiX + `signtool`. Bump RAM to 8 GB if Cargo gets sluggish.
    - Install latest Windows updates before enrollment so the cert
      profile lands on the first sync rather than after a deferred
      reboot cycle.

5. **Entra-join + Intune-enroll the VM.**
    - During OOBE pick *Set up for work or school* → sign in with the
      operator account → Entra join completes → Intune enrollment
      auto-triggers via the org's auto-enrollment policy.
    - In Intune admin center confirm the device shows up under
      *Devices → Windows → Windows devices* with the expected name.
    - Add it to the `cmtraceopen-build-machines` Entra group.

6. **Wait for Intune sync; verify the cert lands.**
    - On the VM: *Settings → Accounts → Access work or school* →
      pick the work account → *Sync*. Or wait ~8 hours for the
      automatic sync.
    - Confirm the cert is present in the LocalMachine personal store:

      ```powershell
      Get-ChildItem Cert:\LocalMachine\My |
        Where-Object { $_.EnhancedKeyUsageList -match 'Code Signing' }
      ```

      Expected output: one cert with subject containing the VM name,
      issuer `CN=issuing.gell.internal.cdw.lab, O=Gell CDW Workspace
      Labs, ...`, EKU `Code Signing (1.3.6.1.5.5.7.3.3)`.

7. **Install the GitHub Actions self-hosted runner.**
    - Repo → *Settings → Actions → Runners → New self-hosted runner*
      → Windows / x64. Follow the per-repo install commands.
    - Run as a Windows service (`./svc.cmd install` then
      `./svc.cmd start`) so the runner survives reboots.
    - Add labels at registration: `self-hosted`, `windows`,
      `cmtrace-build` (the third is what `agent-msi.yml` will target).
    - Reference: <https://docs.github.com/en/actions/hosting-your-own-runners/managing-self-hosted-runners/adding-self-hosted-runners>

8. **Confirm `signtool` can find the cert end-to-end.**
    - Install the Windows SDK (signtool ships under
      `C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe`
      or similar — version may differ). Pin the path in workflow
      env if needed.
    - Smoke test against any throwaway EXE:

      ```powershell
      signtool sign /a /fd sha256 `
        /tr http://timestamp.digicert.com /td sha256 `
        .\test.exe
      signtool verify /pa /v .\test.exe
      ```

      `/a` lets signtool auto-pick the only code-signing cert in the
      LocalMachine store; we deliberately did not name a `/n` filter
      because the strict EKU profile (step 1) guarantees only one
      candidate.

## 5. Reusable signing workflow

The stub at `.github/workflows/sign-agent.yml` becomes a
`workflow_call` reusable workflow targeting the self-hosted runner.
Future `agent-msi.yml` will:

1. Build the unsigned `.msi` (cargo build → WiX harvest → light/candle).
2. Upload it as an artifact.
3. Call `sign-agent.yml` with `artifact-name: CMTraceOpenAgent.msi`
   and a human-readable `description` (used as `signtool /d`).

Inside `sign-agent.yml`:

```yaml
on:
  workflow_call:
    inputs:
      artifact-name:
        required: true
        type: string
      description:
        required: true
        type: string

jobs:
  sign:
    runs-on: [self-hosted, windows, cmtrace-build]
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: ${{ inputs.artifact-name }}
          path: dist
      - name: Sign
        shell: pwsh
        run: |
          & "${env:WINDOWSSDKPATH}\bin\10.0.22621.0\x64\signtool.exe" sign `
            /a /fd sha256 `
            /tr http://timestamp.digicert.com /td sha256 `
            /d "${{ inputs.description }}" `
            "dist\${{ inputs.artifact-name }}"
      - name: Verify chain
        shell: pwsh
        run: |
          & "${env:WINDOWSSDKPATH}\bin\10.0.22621.0\x64\signtool.exe" verify `
            /pa /v /all "dist\${{ inputs.artifact-name }}"
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ inputs.artifact-name }}-signed
          path: dist/${{ inputs.artifact-name }}
```

Notes:

| Step                                | Why                                              |
| ----------------------------------- | ------------------------------------------------ |
| `runs-on: [self-hosted, windows, cmtrace-build]` | Pin to the build VM — only it has the cert |
| `actions/download-artifact@v4`      | Pull the unsigned MSI from the calling workflow  |
| `signtool sign /a`                  | Auto-pick the lone code-signing cert in `LocalMachine\My` |
| `/tr` + `/td sha256`                | RFC 3161 timestamp so signature stays valid past cert expiry |
| `/d "<description>"`                | Stamps user-visible description in UAC prompts   |
| `signtool verify /pa /v /all`       | Fail the run if the chain doesn't validate       |
| `actions/upload-artifact@v4`        | Re-upload the signed MSI for the calling workflow |

Required job permissions: **none** beyond the GitHub default. No
`id-token: write`, no Azure secrets. The cert is local to the
runner; signing is a local syscall, not an API call. This is a
material simplification vs the ATS workflow shape.

## 6. Verification

After signing, `signtool verify` should produce a chain ending at
the Gell PKI Root CA (per `reference_cloud_pki.md`):

```powershell
signtool verify /pa /v /all CMTraceOpenAgent.msi
```

Expected output (abbreviated):

```
Signing Certificate Chain:
  Issued to: gell.internal.cdw.lab          # Gell - PKI Root
    Issued to: issuing.gell.internal.cdw.lab  # Gell - PKI Issuing
      Issued to: <build-vm-name>-codesign
File is signed and timestamped.
Successfully verified: CMTraceOpenAgent.msi
```

The `verify-chain` step in `sign-agent.yml` (above) makes the run
fail loudly if the cert chain ever breaks. Cheap insurance against
a silent expiry or a botched cert-profile renewal.

## 7. Signing cadence

Cloud PKI signing has no per-signature cost — the bill is flat
regardless of how many signatures we mint. That removes ATS's hard
constraint that PR validation MUST NOT sign. We still gate on
release tags for clarity (signed builds are the release artifact;
PRs build unsigned for fast iteration), but the gating is policy,
not cost-driven.

| Trigger                        | Signs?  | Why                                                        |
| ------------------------------ | ------- | ---------------------------------------------------------- |
| Push to `main`                 | No      | Validates build/test only. No release artifact produced.   |
| Pull request                   | No      | Forks can't reach the self-hosted runner anyway. Keeps PR runs on `windows-latest` and fast. |
| Tag `agent-v*` (release)       | **Yes** | Released artifacts get signatures.                         |
| `workflow_dispatch` (manual)   | Optional input | Reserve for ad-hoc dry-runs. Operator opts in via input. |

**Cost benefit (vs ATS):** with ATS we'd be paying ~$0.005 per
signature plus the $15/mo base. With Cloud PKI signing every dev
build during pilot — ~50 signatures/week — costs $0. Flag this as a
real benefit for a project that's still in pilot iteration mode.

## 8. Trust scope

Be explicit about what a Cloud-PKI signature does and does not buy:

- **YES — passes SmartScreen + Intune Win32 publisher trust on every
  Intune-managed device whose tenant has the Cloud PKI root deployed
  in `LocalMachine\Root`.** That's the cmtraceopen pilot fleet by
  design (Cloud PKI's trusted-root distribution rides the MDM
  channel). The signature chains to a CA the device already trusts;
  Defender, SmartScreen, AppLocker, and WDAC all see a publisher
  match.
- **NO — does not pass on a fresh Windows install outside Intune.**
  The Gell PKI Root is a private CA. A laptop that has never enrolled
  in this Intune tenant has no trust path to the cert; SmartScreen
  will warn, AppLocker policies that pin to Microsoft-trusted
  publishers will reject. This is **acceptable for Wave 4** — the
  beta pilot deploys exclusively to managed devices.
- **NO — does not pass for downstream redistributors** (open-source
  consumers, partner orgs without our tenant trust). When that day
  comes, layer ATS on top — see §9.

If you find yourself wanting to sign a binary for an audience
outside the managed fleet, that's the trigger to walk through §9.

## 9. Future broader-release path: Azure Trusted Signing (demoted)

ATS is preserved here as the path for "broader release outside
Intune-managed devices" — open-source distribution, partner orgs
without our tenant trust, public download links. Not Wave 4 scope.

### When to add ATS

Trigger conditions:

- Distributing the agent MSI publicly (e.g., a "download from our
  website" path for non-pilot devices).
- Onboarding a partner org whose Intune tenant doesn't trust the
  Gell PKI Root.
- A user-facing desktop app (vs the headless agent) that runs
  outside an MDM-managed context.

### How to layer it

Append a second signature to the already-Cloud-PKI-signed MSI:

```powershell
# Already signed once with Cloud PKI on the self-hosted runner.
# Append ATS as a second signature.
signtool sign /as `
  /fd sha256 /tr http://timestamp.acs.microsoft.com /td sha256 `
  /dlib "C:\path\to\Azure.CodeSigning.Dlib.dll" `
  /dmdf "C:\path\to\metadata.json" `
  CMTraceOpenAgent.msi
```

The `/as` flag *appends* (does not replace). Both signatures are
valid; verifiers accept either chain. Intune-managed devices verify
via the Cloud PKI chain (already trusted); external devices verify
via the ATS chain (Microsoft public PCA, trusted everywhere).
`signtool verify /pa /v /all` walks both chains and reports both as
valid.

### ATS setup (brief)

The previous version of this doc had the full ATS setup as the
recommended path; the steps remain accurate, just demoted.
Summarized:

1. Create an ATS resource in Azure (region: East US 2 default).
2. Create a Code Signing Account + Public Trust certificate profile.
3. Pass Microsoft Identity Verification (3–7 business days, D&B
   cross-check). **This is the critical-path item if/when ATS is
   added.**
4. Create an Entra app registration with the **Trusted Signing
   Certificate Profile Signer** role on the cert profile.
5. Add a federated credential restricted to release-tag refs.
6. Add a `sign-agent-ats.yml` reusable workflow that runs on
   `windows-latest` (not the self-hosted runner — ATS doesn't need a
   local cert), authenticates via OIDC, and calls
   `azure/trusted-signing-action@v0.5.0`.
7. Update `agent-msi.yml` to call both signing workflows in
   sequence: Cloud PKI first, then ATS append.

Full Microsoft docs (kept here for the day this matters):

- <https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart>
- <https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart-trusted-signing-account-certificate-profile>
- <https://learn.microsoft.com/en-us/azure/trusted-signing/concept-trusted-signing-resources-roles-renewal>

## 10. Sigstore cosign for api-server image (deferred)

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

## 11. Backup plan if Cloud PKI cert profile breaks

Short, because the failure modes are bounded and the recovery is
mechanical.

| Failure                                          | Recovery                                                                              |
| ------------------------------------------------ | ------------------------------------------------------------------------------------- |
| Cert profile fails to renew on the build VM       | Force a sync; if still missing, re-create the Intune cert-profile assignment. Cert lands within ~1 sync cycle. |
| Build VM loses its cert (disk reset, re-image)    | Re-enroll the VM in Intune. Add it back to `cmtraceopen-build-machines`. Wait one sync. |
| Cloud PKI Issuing CA is itself unhealthy (rare)   | Stand up a second build VM in parallel with the same cert profile assignment. Hot-spare. |
| Need to ship today, signing pipeline broken       | **Do not** ship unsigned to Intune-managed devices — SmartScreen warnings on every install. Hold the release. Manual fallback: hand-sign the MSI on the build VM via interactive `signtool` and attach to the GitHub release manually. |

Explicitly **not** a backup plan: skipping signing. Unsigned MSIs
trip SmartScreen and the Intune publisher-trust path on every
managed device — the failure mode is worse than holding a release.

## 12. Open questions

- [ ] **Build VM hosting choice.** Hyper-V on BigMac26 is not viable
  (BigMac is macOS). Options ranked: Azure VM B2s (~$30/mo,
  reproducible via Bicep, network-isolatable via NSG) — recommended.
  On-prem Equinix metal if available — cheapest if hardware exists.
  Hyper-V on a separate Windows host — fine if the host is already
  paid for. Decide before §4 step 4.
- [ ] **Runner labels naming convention.** Current proposal
  `[self-hosted, windows, cmtrace-build]`. If we ever add a second
  build runner (Linux for the api-server image, ARM for testing),
  do we want hierarchical labels (`cmtrace-build-windows-x64`) or
  flat? Decide before runner registration so re-labeling doesn't
  cascade through workflows.
- [ ] **Reproducibility — provision the build VM via Ansible/Bicep?**
  Manual VM standup is fine for one box. If we want the option to
  blow it away and recreate (DR drill, OS upgrade), pin the
  provisioning steps in IaC. Recommended: Bicep for Azure VM path,
  Ansible for on-prem path. Defer until the first DR drill or until
  a second runner is needed.
- [ ] **Timestamp server choice.** Default
  `http://timestamp.digicert.com` is the conservative pick (free,
  long-running, RFC 3161 compliant). Alternates: `http://sha256timestamp.ws.symantec.com/sha256/timestamp`,
  `http://timestamp.acs.microsoft.com` (paired naturally with ATS
  if/when we layer it). Pick one and pin it in the workflow.

---

**Next step:** answer the four questions above, provision the cert
profile + build VM per §4, register the self-hosted runner, and
wire `sign-agent.yml` per §5. Setup is on the order of a day —
much shorter than the 3–7 day ATS Identity Verification that this
path replaces.
