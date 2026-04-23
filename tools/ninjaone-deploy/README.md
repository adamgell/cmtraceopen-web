# NinjaOne deploy — cmtraceopen-agent

Two-piece deploy: a **device-side installer** (lives inside NinjaOne as a saved
script) and a **driver** that invokes it on selected devices via the v2 API.
This split is forced on us because NinjaOne's public API does not expose
script-library CRUD — the library is UI-only. Once the script is saved, the API
can invoke it freely.

## Files

- **`install-cmtrace-agent.ps1`** — runs ON each endpoint under SYSTEM. Downloads
  the signed MSI from the GitHub release, verifies SHA256, `msiexec /i /qn`,
  sanity-checks the service. Idempotent.
- **`Invoke-CmtraceAgentInstall.ps1`** — runs on your Mac. OAuths, resolves the
  script by name, filters the fleet, invokes. Has a `-DryRun` flag.

## One-time setup

### 1. Cut a release tag

The installer downloads the MSI from
`https://github.com/adamgell/cmtraceopen-web/releases/download/agent-v<ver>/CMTraceOpenAgent-<ver>.msi`,
so you need a release to exist.

```sh
git tag -a agent-v0.1.0 -m "cmtraceopen-agent v0.1.0"
git push origin agent-v0.1.0
```

The `agent-msi.yml` workflow signs the MSI and attaches it to the release.

### 2. Save the installer script in NinjaOne

1. Administration → Library → Automation → **New** → **Windows Script**.
2. Fill in:
   - **Name**: `CMTraceOpen Agent — Install`
   - **Description**: `Installs/upgrades cmtraceopen-agent from the signed GitHub release. Idempotent.`
   - **Language**: `PowerShell`
   - **OS**: `Windows`
   - **Architecture**: `Any`
   - **Run As**: `System`
   - **Timeout**: `600` seconds
3. Paste the full contents of `install-cmtrace-agent.ps1` into the script body.
4. Save.

Optional: add Script Parameters so ops can override per-policy without editing
the body:

| Name              | Type   | Default  | Description                   |
| ----------------- | ------ | -------- | ----------------------------- |
| `TargetVersion`   | String | `0.1.0`  | Semver to converge to.        |
| `MsiUrl`          | String | *(blank)* | Override the GitHub release URL. |
| `ExpectedSha256`  | String | *(blank)* | Pin MSI hash; aborts on mismatch. |
| `Reinstall`       | Switch | *(off)*  | Force-repair even if current. |

### 3. Drop your NinjaOne creds on disk

```sh
mkdir -p ~/.config/cmtrace
cat > ~/.config/cmtrace/ninjaone.env <<'EOF'
NINJA_REGION=ca.ninjarmm.com
NINJA_CLIENT_ID=<your-client-id>
NINJA_CLIENT_SECRET=<your-client-secret>
EOF
chmod 600 ~/.config/cmtrace/ninjaone.env
```

The driver reads this automatically. Both values come from a v2-API-scoped
OAuth client you created in Administration → Apps → API.

## Deploying

Dry run first — prints targets without invoking:

```sh
pwsh ./Invoke-CmtraceAgentInstall.ps1 -OrgId 2 -NodeClass WINDOWS -DryRun
```

Real run:

```sh
pwsh ./Invoke-CmtraceAgentInstall.ps1 -OrgId 2 -NodeClass WINDOWS
```

Narrow to one host while testing:

```sh
pwsh ./Invoke-CmtraceAgentInstall.ps1 -HostnameLike gell-e9c0c757
```

## Checking results

NinjaOne surfaces script exit codes on the device's Activities tab. Non-zero
means failure; see the script's stdout in the activity for which stage failed.

On the endpoint itself:

```powershell
Get-Service CMTraceOpenAgent
Get-Content "$env:WINDIR\Temp\CMTraceOpenAgent-install.log" -Tail 40
```

## Why not pass the script content as ad-hoc?

`POST /v2/device/{id}/script` has a "runCustom" shape that accepts PowerShell
content inline. It works, but:

- No audit trail in the Automation library.
- Per-device script body — harder to roll a fleet-wide change.
- Can't tag/run the same script from an automation policy later.

Saving it once in the UI is worth the one-time paste.
