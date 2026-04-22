# cmtraceopen-web

Browser-based log viewer for [CMTrace Open](https://github.com/adamgell/cmtraceopen). Parses logs client-side via WebAssembly — no server required. Companion project to the Tauri desktop app.

## Status

**Wave 3 + Wave 4 are largely shipped.** `cmtraceopen-web` now includes:

### Viewer (browser)
- **Local mode** — drag-drop a CCM/CBS/Panther/etc log → parsed client-side via WebAssembly, no server.
- **API mode** — sign in with Entra, browse devices → sessions → entries from the api-server.
- **Filters + search** — severity multi-select, component contains, message contains with `<mark>` highlighting.
- **Devices view** — operator UI listing registered devices, last-seen, with Admin-gated Disable button.

### API server
- **Ingest** — chunked resumable bundle upload (init → chunks → finalize), parse-on-ingest, byte-identical to desktop parser.
- **Query** — keyset-paginated devices / sessions / entries with operator-bearer (Entra JWKS) auth.
- **Auth** — RBAC (Operator + Admin roles), mTLS termination with cert-derived `DeviceIdentity`, CRL polling for revocation, AppGW header-mode for cloud deploys.
- **Storage** — SQLite or Postgres metadata + local-FS or Azure Blob backend (object_store), Cargo-feature gated.
- **Observability** — Prometheus `/metrics`, structured JSON logs, status page (recent bundles + per-route counters).
- **Operator audit** — append-only audit log of admin actions with keyset cursor query route.
- **Bundle retention** — TTL sweeper (`CMTRACE_BUNDLE_TTL_DAYS`, default 90d) + Azure Blob lifecycle policy alternative.
- **Per-device + per-IP rate limiting** with bounded LRU and trusted-proxy CIDR support.
- **Server-side config push** — operators tune retention/log-level/schedules without re-deploying the agent MSI.

### Agent (Windows)
- **Windows service dispatcher** — proper SCM integration (`CMTraceOpenAgent`, LocalSystem, automatic-delayed-start, restart-on-failure).
- **Collectors** — logs, event logs (wevtutil channels), dsregcmd, evidence orchestrator.
- **Telemetry redaction** — agent-side PII filter (usernames, GUIDs, RFC 1918 IPs, SIDs) with streaming for files >4 MiB.
- **Queue + uploader** — durable on-disk queue, chunked resumable upload, real TLS via rustls + aws-lc-rs.
- **Collection scheduler** — cron + interval modes with per-device deterministic jitter.

### Infra
- **Azure deploy** — Terraform module under `infra/azure/` for Container Apps + Application Gateway with mTLS, Postgres Flexible Server, Storage account, Key Vault.
- **Intune Graph deploy** — `tools/intune-deploy/Deploy-CmtraceAgent.ps1` + `Pack-CmtraceAgent.ps1` for upload + assignment via Graph SDK.
- **WiX MSI** — designed in `docs/wave4/01-msi-design.md` with sources scaffolded under `crates/agent/installer/wix/`.
- **Code signing** — Cloud PKI primary path on a self-hosted GitHub Actions runner; Azure Trusted Signing as the documented broader-release fallback. See `docs/wave4/02-code-signing.md` and `02a-sign-every-component.md`.
- **CI** — `ci.yml` (workspace check/test/clippy + wasm canary + conditional Docker buildx), `audit.yml` (weekly cargo-audit), `semgrep.yml` (security + secrets), `submodule-bump.yml` (weekly cmtraceopen sync), `publish-api.yml` (GHCR on `api-v*` tags), `agent-msi.yml` (Cloud PKI signing on agent-v* tags).

### Ops
- **Day-2 operations runbook** — `docs/wave4/04-day2-operations.md` with 6 incident playbooks, capacity thresholds, DR procedures.
- **Beta pilot runbook** — `docs/wave4/03-beta-pilot-runbook.md`, 14-day 8-device beta with locked success metrics.
- **Quarterly DR rehearsal** — `docs/wave4/20-dr-rehearsal.md` with rotating scenarios (server loss, blob corruption, Cloud PKI outage, PG corruption).
- **Postgres backup automation** — `tools/ops/pg-backup.sh` + `pg-restore.sh` + `blob-backup.sh` (append-only, scratch-DB-only restores).
- **Prometheus alerting** — `infra/observability/prometheus-rules.yaml` + AlertManager + Grafana dashboard.

### Dev stack
- **Local**: `docker compose up` brings up api-server + Postgres + Adminer.
- **BigMac26 runner** (always-on Mac at 192.168.2.50): `dev/bigmac-runner-kit/scripts/redeploy.sh` one-command deploy.
- **End-to-end test**: `cargo test -p api-server --features test-mtls wave4_e2e` exercises Cloud PKI cert → mTLS handshake → bundle ingest → JWT-authenticated query.

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for the developer workflow and [`docs/wave4/`](docs/wave4/) for the full Wave 4 design + operations corpus.

## Prerequisites

- Node.js 20+ (developed on Node 24)
- pnpm 10+ — `corepack enable && corepack prepare pnpm@latest --activate`
- Rust toolchain — managed by `rust-toolchain.toml` (currently pins `1.90`; rustup auto-installs on first build). Add the WASM target manually: `rustup target add wasm32-unknown-unknown`.
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) — prefer the prebuilt installer (`cargo install` rebuilds from source and is slow). On Windows: `winget install rustwasm.wasm-pack`. Cross-platform script: `curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh`.
- The [`cmtraceopen`](https://github.com/adamgell/cmtraceopen) submodule. Cloning fresh:
  ```bash
  git clone --recursive https://github.com/adamgell/cmtraceopen-web.git
  ```
  Or in an existing checkout:
  ```bash
  git submodule update --init --recursive
  ```

## Commands

```bash
pnpm install         # install JS dependencies
pnpm wasm:build      # compile the Rust parser to WASM via wasm-pack
pnpm dev             # wasm:build + start Vite dev server (http://localhost:5173)
pnpm build           # wasm:build + tsc --noEmit + vite production build
pnpm preview         # serve the production build locally
pnpm typecheck       # tsc --noEmit only
```

## Layout

```
cmtraceopen-web/
├── Cargo.toml                 Rust workspace (cmtrace-wasm + crates/api-server + crates/agent + crates/common-wire)
├── rust-toolchain.toml        pins Rust 1.90 across host + Docker builds
├── cmtraceopen/               submodule — Tauri app + cmtraceopen-parser crate
├── cmtrace-wasm/              cdylib crate, wraps cmtraceopen-parser with wasm-bindgen
├── crates/
│   ├── api-server/            Axum HTTP API (ingest, query, status page, mTLS, audit log)
│   │   ├── migrations/        SQLite migrations
│   │   ├── migrations-pg/     Postgres parallel migrations
│   │   └── installer/wix/     (in agent/) WiX v4 MSI sources
│   ├── agent/                 Windows service: collect → queue → ship → sign
│   └── common-wire/           shared DTOs (ingest envelopes, query types)
├── infra/azure/               Terraform for Azure deploy (ACA + AppGW + Postgres + Storage + Key Vault)
├── infra/observability/       PrometheusRules + AlertManager + Grafana dashboard
├── tools/
│   ├── intune-deploy/         Graph SDK script for MSI upload + assignment
│   ├── ops/                   pg-backup, pg-restore, blob-backup
│   ├── ship-bundle.sh         reference ingest client
│   ├── query.sh               operator query helper
│   └── fixtures/build.sh      reproducible test bundle builder
├── pkg/                       wasm-pack output (gitignored)
├── docker-compose.yml         dev stack: api-server + Postgres + Adminer
├── dev/bigmac-runner-kit/     Ansible kit + redeploy.sh for the BigMac26 runner
├── docs/
│   ├── CONTRIBUTING.md        developer workflow
│   ├── adr/                   architecture decisions (Postgres storage types, etc.)
│   ├── provisioning/          Entra app reg + Cloud PKI + Windows VM + Graph deploy runbooks
│   ├── release-notes/         per-version release notes
│   └── wave4/                 24+ design docs: MSI, code-signing, beta pilot, day-2 ops, DR, etc.
├── tests/load/                k6 load tests (bundle-ingest + query-mix scenarios)
└── src/                       Vite + React 19 viewer (Local WASM + API-fetch + Devices admin UI)
```

## Dev status pages

When the full stack is running via `docker compose up`, two debugging UIs are exposed on the host:

- <http://localhost:8080/> — api-server status page: uptime, request counter, build metadata, links to `/healthz` + `/readyz`.
- <http://localhost:8082/> — [Adminer](https://www.adminer.org/) web UI for Postgres. Log in with:
  - System: `PostgreSQL`
  - Server: `postgres`
  - Username: `cmtrace`
  - Password: `cmtrace`
  - Database: `cmtrace`

Both are **dev-only** — no auth, not production-safe. Firewall them off (or drop them from the compose file) before deploying anywhere real.

## Cross-origin requests (CORS)

The api-server ships with a `tower-http` CORS layer applied outermost on the
router, so preflight `OPTIONS` requests are answered before any auth
middleware runs. It's configured via environment variables:

- `CMTRACE_CORS_ORIGINS` — comma-separated list of exact origins permitted to
  call the API from a browser. Default: empty (all cross-origin requests
  rejected — fail closed).
- `CMTRACE_CORS_CREDENTIALS` — `true`/`false` (default `false`). When `true`,
  browsers may attach cookies / `Authorization` headers on cross-origin
  requests.

Typical dev values: `CMTRACE_CORS_ORIGINS=http://localhost:5173,http://localhost:4173`
(Vite dev server + Vite preview). The Vite dev proxy in `vite.config.ts`
remains as a convenience for local development (no CORS round-trip needed),
but **prod deployments** should either serve the viewer same-origin with the
API or set `CMTRACE_CORS_ORIGINS` to the viewer's public origin.

## License

MIT (matches cmtraceopen).
