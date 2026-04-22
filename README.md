# cmtraceopen-web

Browser-based log viewer for [CMTrace Open](https://github.com/adamgell/cmtraceopen). Parses logs client-side via WebAssembly — no server required. Companion project to the Tauri desktop app.

## Status

Phase 2 of the platform split: scaffold + Rust→WASM wrapper. No viewer UI yet — the current build is a hello-world page that loads the WASM parser and confirms it's callable from the browser. Real viewer UI lands in a follow-up.

## Prerequisites

- Node.js 20+ (developed on Node 24)
- pnpm 10+ — `corepack enable && corepack prepare pnpm@latest --activate`
- Rust 1.77+ with the `wasm32-unknown-unknown` target — `rustup target add wasm32-unknown-unknown`
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) — `cargo install wasm-pack`
- A local checkout of [`cmtraceopen`](https://github.com/adamgell/cmtraceopen) as a **sibling directory** (this project depends on `cmtraceopen/crates/cmtraceopen-parser` via a relative path). Both repos live side-by-side, e.g.:
  ```
  F:\Repo\
  ├── cmtraceopen\        (the Tauri desktop app + parser crate)
  └── cmtraceopen-web\    (this repo)
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
├── Cargo.toml             Rust workspace (member: cmtrace-wasm)
├── cmtrace-wasm/          cdylib crate, wraps cmtraceopen-parser with wasm-bindgen
├── pkg/                   wasm-pack output (gitignored)
├── package.json
├── vite.config.ts
├── tsconfig.json
├── index.html
└── src/
    ├── main.tsx
    ├── App.tsx
    └── lib/
        └── wasm-bridge.ts  lazy WASM init + typed parse() wrapper
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
