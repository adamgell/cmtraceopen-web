# Wave 4 — Sign Every Component Inside the MSI

**Status:** Design (not wired up). Layered on top of
[`02-code-signing.md`](02-code-signing.md), which establishes Cloud PKI as
the primary signing identity. This doc describes the **per-artifact**
signing flow: every EXE, DLL, PowerShell script, and the MSI wrapper
itself gets signed in the right order.

**Audience:** anyone implementing `agent-msi.yml` or the MSI build
script; reviewers asked to confirm that the signed-artifact inventory
matches the file inventory in `01-msi-design.md` §4.

**Scope:** signing operations only. The cert-acquisition story (Cloud
PKI vs ATS) and CI auth wiring live in `02-code-signing.md`.

---

## 1. Why component-level signing

Signing only the MSI wrapper is **sufficient for install-time** trust:
Intune Win32 publisher trust passes, SmartScreen shows the recognized
publisher, AppLocker / WDAC publisher rules match. But the install-time
signature does nothing for the **runtime surface**:

- The agent EXE running as a service (`cmtraceopen-agent.exe`) is what
  Defender, EDR, and runtime AppLocker rules look at hour after hour
  once the install has finished.
- Any helper DLLs the agent loads at startup or on demand.
- The MSI custom-action script, if we go the PowerShell path
  (`CertCheck.ps1` per `01-msi-design.md` §6) — PowerShell scripts
  invoked by an MSI custom action are subject to ExecutionPolicy and
  AMSI inspection, and an unsigned script may be blocked by enterprise
  ConstrainedLanguageMode policies.
- Any auxiliary tools we ship inside the MSI (none today, but the
  pattern needs to hold the day we add one).

Modern Defender / EDR / Conditional Access policies that gate on
Authenticode validity at runtime will refuse or warn on each unsigned
component, even though the MSI wrapper is fine. Sign every artifact
for end-to-end chain-of-custody — supply-chain integrity is only as
strong as its weakest unsigned binary.

---

## 2. Inventory of artifacts to sign

Mapped from `01-msi-design.md` §4 (Files installed):

| Artifact                              | Signing tech                          | Notes                                                                                  |
| ------------------------------------- | ------------------------------------- | -------------------------------------------------------------------------------------- |
| `cmtraceopen-agent.exe`               | `signtool sign` (Authenticode)        | Main service binary, Rust release build. KeyPath of the service component.             |
| Bundled DLLs (if any)                 | `signtool sign` (Authenticode)        | Rust release builds are typically statically linked. **Confirm** with `dumpbin /dependents target\release\cmtraceopen-agent.exe` — only system DLLs (`KERNEL32`, `ADVAPI32`, etc.) should appear; if anything from `target\release\deps` shows up, sign it. |
| `CertCheck.ps1` (if PS path)          | `Set-AuthenticodeSignature`           | PowerShell scripts CAN be Authenticode-signed. Required if the operator's tenant enforces AllSigned / ConstrainedLanguageMode. |
| `CertCheck.dll` (if C# DTF path)      | `signtool sign` (Authenticode)        | Same flow as the EXE. Pick this xor the PS option per `01-msi-design.md` §6 / §12 Q2.  |
| `CMTraceOpenAgent.msi`                | `signtool sign` (Authenticode)        | The wrapper. Signed **after** all embedded files.                                      |
| `Cabinet1.cab` (internal)             | covered by MSI wrapper signature      | `signtool sign` on the MSI signs the embedded streams as part of the MSI's storage signature; verify in §6 to confirm.  |

The bundled-DLL row is conditional on the `dumpbin` output. Document
the result in the MSI build's CI log so a reviewer can audit it.

---

## 3. Sign order matters

**Sign EXEs and DLLs and PS scripts FIRST. THEN build the MSI from the
signed inputs. THEN sign the MSI.** Order is non-negotiable: the MSI's
internal cabinet hashes the embedded files; signing files after the MSI
is built would invalidate the MSI's signature on next verify.

```
build agent (cargo build --release)
  → sign cmtraceopen-agent.exe
  → sign helper.dll                   (only if dumpbin shows non-system deps)
  → sign CertCheck.ps1                (only if PS custom-action path)
  → wix build                         (embeds the now-signed files into the MSI)
  → sign CMTraceOpenAgent.msi
  → verify chain on every artifact    (signtool verify + Get-AuthenticodeSignature)
```

If you sign in the wrong order — e.g., sign the MSI first, then sign
the EXE that's already inside it — `signtool verify` on the MSI will
report `SignerHash mismatch` and the install will SmartScreen-warn
even with a valid publisher cert.

---

## 4. signtool invocation per artifact type

### EXE / DLL

```
signtool sign /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 ^
  /d "CMTraceOpen Agent" ^
  target\release\cmtraceopen-agent.exe
```

Flag breakdown:
- `/a` — auto-select the best signing cert from `LocalMachine\My`.
  Filters by EKU = Code Signing automatically. The build VM holds the
  Cloud PKI cert in `LocalMachine\My`; `/a` finds it without a
  thumbprint pin.
- `/fd sha256` — file digest algorithm. SHA-1 is dead; SHA-256 is the
  only acceptable answer in 2026.
- `/tr` — RFC 3161 timestamp authority URL. Without `/tr` the
  signature stops being valid the day the cert expires; with `/tr` the
  signature stays valid forever (the timestamp proves the file was
  signed *while* the cert was valid).
- `/td sha256` — timestamp digest algorithm. **Must match `/fd`** for
  FIPS-aligned configs to accept the signature; mixing SHA-1 timestamp
  with SHA-256 file digest is a common gotcha that fails on hardened
  endpoints.
- `/d "..."` — description shown in the UAC prompt and Windows
  certificate-info dialogs. Make it human-readable; this is what
  operators see when triaging a SmartScreen prompt.

### PowerShell script

PowerShell scripts use the built-in cmdlet, not signtool:

```powershell
$cert = Get-ChildItem Cert:\LocalMachine\My |
  Where-Object { $_.EnhancedKeyUsageList.FriendlyName -contains 'Code Signing' } |
  Select-Object -First 1

Set-AuthenticodeSignature -Certificate $cert `
  -FilePath .\crates\agent\installer\wix\CustomActions\CertCheck.ps1 `
  -TimestampServer http://timestamp.digicert.com `
  -HashAlgorithm SHA256
```

Same cert, same timestamp authority, same SHA-256 digest as the EXE
flow. The signature is appended to the script as a `# SIG # Begin
signature block` comment block — the script itself remains
human-readable above the signature.

### MSI (wrapper)

```
signtool sign /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 ^
  /d "CMTraceOpen Agent Installer" ^
  out\CMTraceOpenAgent-{version}.msi
```

Identical to the EXE invocation. The only difference is `/d`: the
installer description matters more — this is the string operators see
in the SmartScreen "Do you want to allow this app to make changes?"
prompt.

---

## 5. CI workflow integration

How the future `agent-msi.yml` (stubbed in PR #58) wires this in.
Pseudocode YAML:

```yaml
- name: Build agent (release)
  run: cargo build --release -p agent

- name: Sign agent.exe
  shell: pwsh
  run: |
    & signtool.exe sign /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 `
      /d "CMTraceOpen Agent" `
      .\target\release\cmtraceopen-agent.exe

- name: Sign CertCheck.ps1 (if PS custom action)
  shell: pwsh
  run: |
    $cert = Get-ChildItem Cert:\LocalMachine\My |
      Where-Object { $_.EnhancedKeyUsageList.FriendlyName -contains 'Code Signing' } |
      Select-Object -First 1
    Set-AuthenticodeSignature -Certificate $cert `
      -FilePath .\crates\agent\installer\wix\CustomActions\CertCheck.ps1 `
      -TimestampServer http://timestamp.digicert.com `
      -HashAlgorithm SHA256

- name: Build MSI
  shell: pwsh
  run: |
    .\crates\agent\installer\wix\build.ps1 `
      -ReleaseBinary .\target\release\cmtraceopen-agent.exe `
      -Version $env:VERSION

- name: Sign MSI
  shell: pwsh
  run: |
    & signtool.exe sign /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 `
      /d "CMTraceOpen Agent Installer" `
      .\out\CMTraceOpenAgent-$env:VERSION.msi

- name: Verify all signed artifacts
  shell: pwsh
  run: |
    & signtool.exe verify /pa /v /all .\target\release\cmtraceopen-agent.exe
    Get-AuthenticodeSignature .\crates\agent\installer\wix\CustomActions\CertCheck.ps1 |
      Format-List Status, SignerCertificate, TimeStamperCertificate
    & signtool.exe verify /pa /v /all .\out\CMTraceOpenAgent-$env:VERSION.msi
```

The build VM is the only place the Cloud PKI signing cert lives, in
`LocalMachine\My`. `signtool /a` auto-selects it. There is no secret
to inject and no thumbprint to pin in repo config — see
`02-code-signing.md` for the cert-provisioning story.

---

## 6. Verification

After signing, confirm with three commands. Each should report
"Successfully verified" and a chain ending at
`Gell - PKI Issuing CA` → `Gell - PKI Root CA`
(the Cloud PKI hierarchy provisioned per
[`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md);
the Issuing CA's EKU set includes Code Signing, OID 1.3.6.1.5.5.7.3.3):

```powershell
signtool verify /pa /v /all CMTraceOpenAgent.msi
signtool verify /pa /v /all cmtraceopen-agent.exe
Get-AuthenticodeSignature .\CertCheck.ps1 | Format-List Status, SignerCertificate, TimeStamperCertificate
```

`signtool verify /pa /v /all` flag breakdown:
- `/pa` — use the Authenticode policy (vs the default Windows driver
  policy, which is wrong for application binaries).
- `/v` — verbose; prints the full cert chain so a reviewer can eyeball
  it.
- `/all` — verify *every* signature on the file (relevant once the ATS
  counter-signature lands, see §8).

For the PowerShell script, `Get-AuthenticodeSignature` is the right
tool. Status field interpretation:
- `Valid` — script is signed, cert chains to a trusted root, hash
  matches. Ship it.
- `NotSigned` — the script wasn't signed. Sign step was skipped or
  failed silently.
- `HashMismatch` — the script was modified after signing. Common cause:
  CRLF/LF line-ending changes from a post-sign editor save. Re-sign.
- `UnknownError` — usually means the cert chain doesn't validate on
  the verifying machine. Cross-check trust (the Cloud PKI root must be
  in `LocalMachine\Root` on the verifier).

---

## 7. Runtime verification (optional, recommended)

Defense in depth: the agent service can self-verify its own signature
at startup. If the build VM is taken over by an attacker who swaps
`cmtraceopen-agent.exe` on disk, the swapped binary won't have a
matching signature and the service refuses to start.

The `windows-sys` crate exposes `WinVerifyTrust`. Sketch (not for this
PR — listed as deferred work):

```rust
// At the top of fn main(), before the service dispatcher starts:
fn verify_self_signature() -> Result<()> {
    let exe = std::env::current_exe()?;
    // Build a WINTRUST_DATA struct pointing at exe, call WinVerifyTrust
    // with WINTRUST_ACTION_GENERIC_VERIFY_V2. Return Err on non-zero.
    // ...
}
```

Mark this as **P2 (deferred)** for v1. The install-time signature
check (Defender + AppLocker + WDAC, all running before the service is
started) is the primary integrity guarantee. Self-verify is belt and
suspenders, useful once the agent is doing high-trust operations
(remote command execution, etc.) — none of which v1 ships.

---

## 8. Counter-signing for future ATS layer

When the future Azure Trusted Signing path lands (per
`02-code-signing.md` §3 once it's rewritten to scope ATS as the
external-trust layer), append the ATS signature with `/as` (append
signature). Both signatures coexist on the same file: Cloud PKI for
internal/Intune trust, ATS for SmartScreen / external-trust paths.
Verifiers accept either signature.

```
signtool sign /as /a /fd sha256 /tr http://timestamp.digicert.com /td sha256 ^
  /d "CMTraceOpen Agent (ATS counter-sign)" ^
  /sha1 <ATS-cert-thumbprint> ^
  dist\CMTraceOpenAgent.msi
```

`/as` is the **append signature** flag — without it, signtool would
*replace* the existing Cloud PKI signature instead of stacking. Once
both sigs are present, `signtool verify /pa /v /all` enumerates both
and the verifier picks whichever one chains to a root it trusts.

---

## 9. Trust scope reminder

Same caveat as `02-code-signing.md` §8 (Trust scope): the Cloud PKI
signature passes Authenticode validation on every Intune-managed
device whose tenant trusts the Cloud PKI root — which is every
device the cert profile has been deployed to. Outside that
boundary (a developer laptop without the root installed, an Intune
tenant in a different org, a non-managed Windows Server VM) the
signature is *present* but not *chain-validated* — SmartScreen will
fall back to "unknown publisher" and Defender will treat the binary
as unsigned.

Acceptable for the pilot. Once the ATS counter-signature lands per
§8, the externally-trusted signature handles those out-of-tenant
cases.

---

## 10. Open questions

1. **Custom-action language (PS vs DTF).** Determines whether one or
   two signing flows are needed. PS path adds the
   `Set-AuthenticodeSignature` step; DTF path collapses to "everything
   is signtool." Cross-reference `01-msi-design.md` §6 and §12 Q2 —
   this is the same open question, viewed from the signing angle.

2. **Sign release-binary archives too.** The MSI is the supported
   install path, but we may attach `agent-vX.Y.Z.zip` (raw EXE +
   README) to GitHub Releases for offline / non-Intune install paths.
   Probably yes — sign the contents (the EXE inside is already signed
   per §3) and consider signing the .zip wrapper itself, though .zip
   is not a PE container so signtool won't touch it; an `.intunewin`
   wrapper or a detached `.zip.sig` would be the alternatives. Defer
   the decision until a non-Intune consumer asks for it.

3. **Bundled DLL inventory.** §2 assumes Rust release builds are
   statically linked and `dumpbin /dependents` shows only system DLLs.
   Confirm this in the first MSI build's CI log; if any
   `target\release\deps\*.dll` ships, add them to the sign-list and
   update §2.

---

**See also:** `02-code-signing.md` for the cert acquisition / CI auth
story, `01-msi-design.md` §4 for the file inventory this signing flow
covers, [`docs/provisioning/03-intune-cloud-pki.md`](../provisioning/03-intune-cloud-pki.md)
for the Cloud PKI hierarchy (Gell - PKI Root CA → Gell - PKI Issuing CA;
Issuing CA EKU includes Code Signing, OID 1.3.6.1.5.5.7.3.3) that
signatures chain to.
