# cmtraceopen-web

Browser-based log viewer for [CMTrace Open](https://github.com/adamgell/cmtraceopen). Parses logs client-side via WebAssembly — no server required. Companion project to the Tauri desktop app.

## Status

Walking skeleton is live. `cmtraceopen-web` ships:

- **Viewer** — drag-drop WASM mode and API-fetch mode (device → session → entries) over the api-server.
- **API server** — Axum + SQLite + local-FS blob store; chunked resumable bundle ingest, parse-on-ingest, entries/files queries.
- **Agent** (Windows) — collectors for logs / event logs / dsregcmd, queues bundles, ships over HTTPS.
- **Dev stack** — `docker compose up` brings up api-server + Postgres + Adminer; `dev/bigmac-runner-kit/scripts/redeploy.sh` one-command deploys to the always-on Mac runner.

Wave 2/3 in flight: operator OAuth (Entra), real CORS, mTLS termination, RBAC, CRL polling, Azure Blob, Prometheus metrics, GHCR publish.

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) for the full developer workflow.

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
├── Cargo.toml             Rust workspace (cmtrace-wasm + crates/api-server + crates/agent + crates/common-wire)
├── rust-toolchain.toml    pins Rust 1.90 across host + Docker builds
├── cmtraceopen/           submodule — Tauri app + cmtraceopen-parser crate
├── cmtrace-wasm/          cdylib crate, wraps cmtraceopen-parser with wasm-bindgen
├── crates/
│   ├── api-server/        Axum HTTP API (ingest, query, status page)
│   ├── agent/             Windows service: collect → queue → ship
│   └── common-wire/       shared DTOs (ingest envelopes, query types)
├── pkg/                   wasm-pack output (gitignored)
├── docker-compose.yml     dev stack: api-server + Postgres + Adminer
├── dev/bigmac-runner-kit/ Ansible kit + redeploy.sh for the BigMac26 runner
├── tools/                 ship-bundle.sh + query.sh + fixtures/build.sh
├── docs/                  CONTRIBUTING + provisioning runbooks + release.md
└── src/                   Vite + React 19 viewer (drag-drop WASM + API-fetch modes)
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
