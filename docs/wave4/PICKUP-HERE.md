# Wave 4 — Pickup Here

Last updated: **2026-04-22**.

> Bookmark for resuming Wave 4 work without re-deriving where we left off.
> Updated each session that moves the rollout forward.

---

## Where we are

Wave 3 + Wave 4 **design** is fully shipped. Wave 4 **execution** has just
started. The walking skeleton is live on BigMac26 and the api-server is now
published to GHCR.

### Shipped this session (2026-04-22)

- 58 PRs merged completing the Wave 3 surface (mTLS, RBAC, CRL polling,
  Prometheus, Azure Blob, agent service dispatcher, redaction, scheduler,
  Postgres scaffold, audit log, rate limiting, server-side config push,
  AppGW header mode, Devices admin UI, k6 load tests, e2e tests) and the
  Wave 4 design surface (code-signing pipeline, Azure ACA + AppGW
  Terraform module, Intune Graph deploy script, beta pilot / day-2 ops /
  DR runbooks).
- **`api-v0.1.0` tag cut** → first GHCR publish at
  `ghcr.io/adamgell/cmtraceopen-api:0.1.0` (multi-arch
  linux/amd64 + linux/arm64).
- **`docker-compose.yml` defaults to GHCR pull** (PR #113). `--build`
  opt-in retained; `CMTRACE_API_IMAGE` env override for testing RC tags.
- **BigMac26 redeployed** off the new compose — pulling published image
  instead of building from source.

---

## What's actively in flight

### Build VM + self-hosted runner

User started provisioning the Wave 4 build VM and registered a GitHub
Actions self-hosted runner against `adamgell/cmtraceopen-web`.

- **Status:** runner installed; was running as `NETWORK SERVICE`; user is
  switching it to a dedicated local service account so `git` works
  (Network Service has no `USERPROFILE`/`HOME`, breaks credential
  helpers, SSH, and submodules).
- **Why a build VM at all:** Cloud PKI code-signing certs only land in
  `LocalMachine\My` on **Intune-enrolled** Windows hosts. GitHub-hosted
  runners aren't enrolled, so they can't sign. See
  [`02-code-signing.md`](./02-code-signing.md) §4 and
  [`07-build-vm-runbook.md`](./07-build-vm-runbook.md).
- **Recipe:** [`07-build-vm-runbook.md`](./07-build-vm-runbook.md) is the
  authoritative runbook; [`../provisioning/04-windows-test-vm.md`](../provisioning/04-windows-test-vm.md)
  covers the VM-level pieces.

---

## What unblocks next, in order

Once the runner can run `git`:

1. **Verify the Cloud PKI cert actually landed** in `LocalMachine\My`:
   ```powershell
   Get-ChildItem Cert:\LocalMachine\My |
     Where-Object { $_.Issuer -like '*Gell*Issuing*' -and
                    $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3' }
   ```
   Issuer chain: `Gell - PKI Root` → `Gell - PKI Issuing` (see
   `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md`).
   Grant the runner service account Read on the private key (MMC →
   Certificates → All Tasks → Manage Private Keys).
2. **Land WiX MSI sources** under `crates/agent/installer/wix/` per
   [`01-msi-design.md`](./01-msi-design.md). Fixed UpgradeCode
   `463FD20A-1029-448F-AE5B-F81C818861D0`.
3. **Wire `agent-msi.yml`** workflow that builds the MSI then calls the
   already-scaffolded `sign-agent.yml` — both live under
   `.github/workflows/`.
4. **First signed MSI** through the pipeline.
5. **Pack** via `tools/intune-deploy/Pack-CmtraceAgent.ps1` →
   `.intunewin` artifact. Pin `IntuneWinAppUtil.exe` SHA256.
6. **Deploy** via `tools/intune-deploy/Deploy-CmtraceAgent.ps1` (Graph
   SDK) → uploads to Intune, assigns to pilot device group.
7. **Stand up internet-reachable api-server** before pilot — BigMac26 is
   corp-LAN-only and devices off-LAN can't reach it. Recommended: Azure
   Container Apps + Application Gateway via `infra/azure/` Terraform
   module ([`05-azure-deploy.md`](./05-azure-deploy.md)). Beta blocker
   per [`03-beta-pilot-runbook.md`](./03-beta-pilot-runbook.md).
8. **Beta pilot** — 8 devices, 14 days, 5 success metrics
   ([`03-beta-pilot-runbook.md`](./03-beta-pilot-runbook.md)).

---

## Adjacent followups (not on the critical path)

- **Viewer UX polish** — search/filter polish exists; severity pivot,
  cross-file timeline, saved queries are open ideas.
- **Postgres migration** — scaffold is live (`migrations-pg/` + parity
  test); SQLite is still the dev default. Switch when load actually
  warrants ([`10-postgres-migration.md`](./10-postgres-migration.md)).
- **Bundle retention** — design done
  ([`08-bundle-retention.md`](./08-bundle-retention.md)); cron-based
  cleanup not yet wired.
- **Cross-device fleet search** (`/v1/search`) — out of v1 scope; design
  pass once query patterns are known from real beta data.

---

## Repos + machines cheat sheet

| Where | What |
|---|---|
| `F:\Repo\cmtraceopen-web` | platform + viewer (this repo) |
| `F:\Repo\cmtraceopen` | desktop app + parser crate (submodule) |
| `BigMac26` (`192.168.2.50`) | dev api-server + Postgres + adminer (`:8082`); now pulling GHCR `0.1.0` |
| `ghcr.io/adamgell/cmtraceopen-api` | published api-server image; tag on `api-v*` push |
| Build VM (in progress) | self-hosted runner + Cloud PKI signing cert holder |

## Useful scripts

- `dev/bigmac-runner-kit/scripts/redeploy.sh` — redeploy api-server to BigMac
- `tools/fixtures/build.sh && tools/ship-bundle.sh` — round-trip ingest test
- `tools/intune-deploy/Pack-CmtraceAgent.ps1` — package MSI into .intunewin
- `tools/intune-deploy/Deploy-CmtraceAgent.ps1` — Graph upload + assign
