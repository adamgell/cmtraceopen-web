# Contributing to cmtraceopen-web

Welcome. This document is the field guide for a fresh contributor (or a
Claude session in six months) coming in cold. It captures the gotchas
learned during the build-out so you don't re-derive them. For the
system-level "what is this" picture, read
[`docs/architecture.md`](./architecture.md) first; this doc assumes you
already know roughly what the project is.

## Table of contents

1. [Prereqs](#prereqs)
2. [Repo layout](#repo-layout)
3. [Common workflows](#common-workflows)
4. [Gotchas](#gotchas-the-long-list)
5. [PR conventions](#pr-conventions)
6. [Where things live (memory + docs)](#where-things-live-memory--docs)
7. [Where to ask](#where-to-ask)

---

## Prereqs

Install once per machine. Versions are floors, not ceilings.

- **Node 20+** with **pnpm** via Corepack. Don't `npm i -g pnpm` — let
  Corepack pin the version from `package.json#packageManager`:
  ```bash
  corepack enable
  corepack prepare pnpm@latest --activate
  ```
- **Rust 1.90+**. You don't need to install this manually — `rustup`
  auto-installs the channel pinned in
  [`rust-toolchain.toml`](../rust-toolchain.toml) the first time you run
  `cargo` in the repo. If you don't have rustup yet:
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **wasm-pack**. Use the pre-built installer, not `cargo install`. The
  `cargo install` path drags in `ring`, which has a clang dep that is
  miserable on ARM64 Windows:
  ```bash
  curl -sSfL https://rustwasm.github.io/wasm-pack/installer/init.sh | sh
  wasm-pack --version
  ```
  This is what CI does — see
  [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).
- **Docker**. Colima on Mac (`brew install colima docker`), Docker
  Desktop on Windows. If you ever build the api-server image locally
  (not just `docker compose up`), you'll need `cmake` + `nasm` available
  to the builder for `aws-lc-rs`. Inside the official `rust:slim` base
  image this Just Works; on a host build you'll need them on `$PATH`.
- **Submodule init** after clone — the parser crate lives in the
  `cmtraceopen` submodule and both api-server and cmtrace-wasm depend on
  it:
  ```bash
  git clone https://github.com/adamgell/cmtraceopen-web.git
  cd cmtraceopen-web
  git submodule update --init --recursive
  ```

If you skip the submodule step, `cargo check` succeeds (api-server's
parser dep resolves through the relative path to an empty directory,
which Cargo treats as a hard error eventually) but `docker compose
up --build` fails inside the container with a "could not find
`cmtraceopen-parser`" error during the `COPY . .` stage. See the
[`.dockerignore`](#gotchas-the-long-list) gotcha below.

---

## Repo layout

Brief tour. The diagram and protocol summary live in
[`docs/architecture.md`](./architecture.md) (added in PR #31) — treat
this section as a name-only index.

```
cmtraceopen-web/
├── crates/
│   ├── api-server/       Axum HTTP API (:8080), ingest + query routes
│   ├── agent/            Windows service skeleton + collectors
│   └── common-wire/      Shared DTOs (serde camelCase, no business logic)
├── cmtrace-wasm/         wasm-bindgen wrapper around the parser crate
├── cmtraceopen/          Submodule — the Tauri app repo, source of the parser
├── src/                  Vite + React viewer (TypeScript)
│   ├── components/       ViewerShell, LocalMode, ApiMode, EntryList, ...
│   └── lib/              api-client.ts, wasm-bridge.ts, log-types.ts
├── docs/
│   ├── architecture.md       system diagram + protocol
│   ├── provisioning/         Windows VM, Entra app reg, Intune Cloud PKI
│   └── release.md            release process notes
├── tools/                ship-bundle.sh, query.sh, fixtures/
├── dev/bigmac-runner-kit/  Ansible kit + redeploy.sh for the LAN runner
├── scripts/              wasm-smoke.mjs (CI parser regression)
├── docker-compose.yml    api-server + Postgres + Adminer (dev stack)
├── Cargo.toml            workspace root (excludes cmtrace-wasm — see gotchas)
└── rust-toolchain.toml   pins rustc channel
```

The cross-cutting concept: `cmtraceopen-parser` is the **only** code
shared with the desktop app. api-server pulls it in natively through a
path dep; cmtrace-wasm re-exports it through `wasm-bindgen` for the
browser's local drag-drop mode. One parser, two call sites.

---

## Common workflows

Every command below is copy-pasteable from the repo root.

### Build everything locally (sanity check)

```bash
pnpm install
pnpm build              # wasm:build + tsc --noEmit + vite build
cargo check --workspace
```

This is the minimum you should run before pushing a PR.

### Run the dev viewer

```bash
pnpm dev
```

Vite starts on `http://localhost:5173` and proxies `/v1` and `/healthz`
to `http://localhost:8080`, so the browser sees a single origin and the
preflight round-trip is skipped. Configured in
[`vite.config.ts`](../vite.config.ts).

### Run the dev api-server (no Docker)

```bash
CMTRACE_AUTH_MODE=disabled cargo run -p api-server
```

Defaults: listens on `127.0.0.1:8080`, data dir `./data` (SQLite +
blobs), no Entra. The `CMTRACE_AUTH_MODE=disabled` is mandatory — the
server refuses to start if it can't find Entra config AND auth isn't
explicitly disabled.

### Run the full Docker stack

```bash
docker compose up --build
```

- `api-server` on `:8080`
- `postgres` on `:5432` (host-exposed for ad-hoc psql)
- `adminer` on `:8082` (Postgres web UI; was `:8081` before PR #12,
  see gotchas)

The first build is ~90 seconds; subsequent builds use the cargo cache
mounts in the Dockerfile.

### Smoke-test ingest

```bash
bash tools/fixtures/build.sh
bash tools/ship-bundle.sh \
  --endpoint http://localhost:8080 \
  --device-id dev-01 \
  --bundle tools/fixtures/test-bundle.zip
```

`tools/fixtures/test-bundle.zip` is gitignored — always rebuild before
shipping. See [`tools/README.md`](../tools/README.md) for the resume
path, error semantics, and a query.sh recipe.

### Deploy to BigMac26

```bash
./dev/bigmac-runner-kit/scripts/redeploy.sh \
  -i dev/bigmac-runner-kit/id_ed25519
```

The `-i` arg is optional; defaults to `~/.ssh/id_ed25519`. SSHes to
`192.168.2.50`, pulls the named branch (default `main`), runs
`docker compose down && up -d --build`, then runs `/healthz` + `/readyz`
smoke tests. Flags: `--branch`, `--skip-build`, `--no-smoke`. See
[`dev/bigmac-runner-kit/scripts/redeploy.sh`](../dev/bigmac-runner-kit/scripts/redeploy.sh)
for the full source.

---

## Gotchas (the long list)

These are the sharp edges. If you're debugging something weird, check
here first.

### No `ring` crate — anywhere

Project rule: nothing transitive may pull in `ring`. The reason is
ARM64 Windows: `ring` requires a working clang and the assembly path
fights toolchain detection. We use `rustls` + `aws-lc-rs` instead.
`aws-lc-rs` has its own build-time deps (cmake, nasm) but they're
trivial to install on Linux and CI, and the Rust-side experience is
clean across all our targets (Linux, macOS, Windows x64, Windows
ARM64).

Verify a crate is ring-free:

```bash
cargo tree -p api-server | grep ring   # should print nothing
cargo tree -p agent      | grep ring
```

If a new dep pulls in `ring`, find a `default-features = false` knob or
swap the dep — don't add it to the tree. PR #29 (`reqwest` switched to
`native-tls` to dodge ring) and PR #37 (agent's rustls + aws-lc-rs
setup) are the precedents.

### `rust-toolchain.toml` is the single source of truth for rustc

Pinned to `1.90` today. When a transitive dep needs newer rustc (e.g.
`jwt-simple` → `aes-keywrap` → `unsigned_is_multiple_of`, stabilized in
1.90), bump the toolchain file — **not** the CI workflow, **not**
individual Dockerfiles. `rustup` honors the pin on every `cargo`
invocation and auto-installs the channel if missing.

### `actions/checkout` with `submodules: true`, NOT `recursive`

The `cmtraceopen` submodule used to track some accidental gitlinks
under `.claude/worktrees/*` (Claude Code worktree machinery). Those
aren't real submodules — there's no `.gitmodules` entry — so `--recursive`
checkout dies on them. Workflows use `submodules: true` (one level) and
that's enough for our use case (we only need the parser crate).

The bad gitlinks were cleaned up in cmtraceopen, but the rule still
applies as a defense-in-depth measure: if any `.claude/worktrees/`
gitlinks sneak back in via a future merge, `submodules: true` keeps CI
green.

### Don't add `/cmtraceopen` to `.dockerignore`

[`.dockerignore`](../.dockerignore) deliberately omits the submodule.
api-server's parse-on-ingest worker depends on `cmtraceopen-parser`
through a relative path (`cmtraceopen/crates/cmtraceopen-parser`), and
that path is only satisfied if the submodule directory is in the build
context. The submodule is pure Rust (no Tauri / no native UI deps) so
it compiles cleanly inside the distroless toolchain.

If you ever see `error: could not find cmtraceopen-parser in
registry crates.io` during a Docker build, the cause is one of:
- forgot `git submodule update --init --recursive` on the host
- accidentally added `/cmtraceopen` to `.dockerignore`

### Distroless `/data` — leave the `dataprep` stage alone

The api-server runtime image is `gcr.io/distroless/cc-debian12` running
as `nonroot` (UID/GID 65532). Distroless has no shell, no `mkdir`, no
`chown`, so we can't create the blob store dir from inside the runtime
stage. The `dataprep` stage in
[`crates/api-server/Dockerfile`](../crates/api-server/Dockerfile)
exists to pre-create `/data` with the right ownership, then `COPY
--from=dataprep --chown=65532:65532 /data /data` into runtime. Do not
remove that stage thinking it's redundant — the container will fail to
write blobs without it.

### Vite dev proxy vs. production CORS

In dev, the Vite server (`localhost:5173`) proxies `/v1` and `/healthz`
to `localhost:8080` so the browser sees one origin. **Production has no
proxy.** Set `CMTRACE_CORS_ORIGINS` (comma-separated, exact origins) on
the api-server to the viewer's public URL. CORS layer landed in PR #26
and is wired outermost on the router so preflight `OPTIONS` requests
answer before any auth middleware runs. Default is empty (fail closed).

### Auth dev mode — `CMTRACE_AUTH_MODE=disabled`

The api-server requires either:
- a complete Entra config (`CMTRACE_ENTRA_TENANT_ID`,
  `CMTRACE_ENTRA_AUDIENCE`, `CMTRACE_ENTRA_JWKS_URI`), or
- `CMTRACE_AUTH_MODE=disabled` set explicitly

`docker-compose.yml` sets the disabled value by default (PR #42) so the
local stack just works. **Production deployments must remove that env
var and provide real Entra config** — otherwise the api accepts any
caller. The compose file has a comment to this effect; don't strip it.

### `X-Device-Id` header is a temporary placeholder

Today's ingest path identifies the device by an `X-Device-Id` HTTP
header. This is the MVP wire. Wave 3 swaps it for mTLS termination at
the api-server, with device identity parsed from the client cert SAN
URI (`device://{tenant}/{aad-device-id}`). When you see `X-Device-Id`
in the codebase, treat it as a TODO marker for the mTLS swap. Don't
build new long-term features on top of it; design them to work post-
mTLS too. Cert issuance plan: [`docs/provisioning/03-intune-cloud-pki.md`](./provisioning/03-intune-cloud-pki.md).

### MSAL.js bundle size

Adding `@azure/msal-browser` + `@azure/msal-react` for operator OAuth
grew the viewer bundle by ~60 kB gzipped. That's acceptable today. If
the bundle grows further (new auth flows, additional MSAL extensions),
consider lazy-loading the auth path so the unauthenticated landing
shell stays small. Don't pre-emptively reorganize for it now — wait for
a real metric to push back.

### Submodule pointer bumps — be deliberate

`cmtraceopen` is pinned to a specific commit in `.gitmodules`. CI
honors the pin (hermetic). When the parser advances upstream (parser
bug fix, new content kind, etc.), bump the pointer here:

```bash
git -C cmtraceopen pull --ff-only
git add cmtraceopen
git commit -m "chore: bump cmtraceopen submodule to <short-sha>"
```

**Automated bumps:** the
[`submodule-bump` workflow](../.github/workflows/submodule-bump.yml)
runs every Tuesday at 14:00 UTC and opens a PR automatically when
`cmtraceopen`'s upstream `main` has moved ahead of the pinned pointer.
The PR title follows the pattern
`chore: bump cmtraceopen submodule from <oldsha> to <newsha>` and the
body includes the upstream changelog. A PR is only opened when the
pointer has actually changed; if an open bump PR already exists the
workflow skips to avoid duplicates. You can also trigger it on-demand
via **Actions → Bump cmtraceopen submodule → Run workflow**.

> **Prerequisites:** the workflow requires a `PAT_TOKEN` repository
> secret (a Personal Access Token with `repo` scope). Without it the
> workflow fails immediately with an actionable error message. Add it
> under **Settings → Secrets and variables → Actions → New repository
> secret**.

> **If a duplicate bump PR appears** (e.g. after a reviewer
> renames/rebases the branch), close or merge the stale PR and re-run
> the workflow. The idempotent guard matches on the branch name
> `chore/bump-cmtraceopen-submodule`; any PR on a different branch will
> not be detected as a duplicate.

Each bump is a discrete commit so it's easy to revert if the parser
introduces a regression. The CI parser-regression canary
([`scripts/wasm-smoke.mjs`](../scripts/wasm-smoke.mjs)) catches drift
between the WASM build and the desktop parser; if it fails on a bump,
the parser changed and you need to update the expected counts.

### Windows ARM64 dev box quirks

- `cargo install wasm-pack` fails because of `ring`. Use the pre-built
  installer (see [Prereqs](#prereqs)) or `winget install
  LLVM.LLVM` to get clang on `$PATH` first if you really want
  `cargo install`.
- The cargo target dir on a Windows host can clash with the cargo cache
  inside the api-server Docker build. If you see weird linker errors
  after switching between native and Docker builds, run `cargo clean`
  on the host before retrying.

### BigMac26 host port `:8081` is taken

An mlx-vlm Python server was already squatting on `:8081` on BigMac26.
PR #12 moved Adminer from `:8081` → `:8082` to avoid the collision.
If you ever change the Adminer port back, double-check the host first
(`lsof -i :8081` over SSH).

### `target/` directories are NOT shared between workspaces

There are two Rust workspaces in this repo:

1. The root workspace (`Cargo.toml`) — `api-server`, `common-wire`,
   `agent`. Builds to `./target/`.
2. `cmtrace-wasm/` — its own workspace because it has a cross-repo path
   dep on the parser. Builds to `cmtrace-wasm/target/`.

Both are cached separately in CI. If you blow away one `target/` for
disk space, you don't have to blow away the other. Conversely,
"`cargo clean`" only cleans the workspace you're in — to wipe both, run
it from each root.

---

## PR conventions

- **Conventional Commits.** Prefix the title with `feat:`, `fix:`,
  `chore:`, `docs:`, `refactor:`, `test:`, `ci:`. Keep titles under 70
  chars; put context in the body.
- **`Co-Authored-By` footer for AI-assisted commits.** Current pattern
  is the Claude footer:
  ```
  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  ```
- **Draft PRs by default.** Open as draft, flip to ready when CI is
  green and you're comfortable with the diff. Reviewers (currently
  just you, future contributors when they show up) won't be paged
  before that.
- **Force-merge with `--admin --merge` is OK.** This is a team-of-one
  project today; the branch protection exists to keep CI honest, not
  to gate solo merges. Revisit when collaborators arrive.
- **Stack PRs only when dependencies require it.** If PR B literally
  cannot exist without PR A's wire format change, stack them (B's base
  branch is A). Otherwise prefer independent PRs against `main` so
  rebases don't cascade. Rebase agents handle merge-conflict mechanics
  if a stack does form.
- **Don't push to `main` directly.** Even for trivial changes — branch,
  PR, merge. The audit trail is cheap.

---

## Where things live (memory + docs)

The non-obvious but high-signal places to look when context is missing.

### Claude session memory

`~/.claude/projects/F--Repo/memory/` — local-machine session notes
written by previous Claude sessions. **Read these first** when picking
up a new task; they explain "why" decisions that aren't documented in
the code:

- `project_walking_skeleton.md` — what's actually shipped on `main`
  today (vs. what's in flight)
- `reference_cloud_pki.md` — the real CA chain we're targeting for mTLS
- `feedback_parallel_agents.md` — the project owner's preferences
  around orchestration style (parallel agents, draft-PR workflow,
  rebase agents, etc.)

The seven architectural decisions that shaped the project (no ring,
single parser, two workspaces, fail-closed CORS, etc.) are reproduced
in the memory files and the architecture plan; this doc deliberately
does not repeat them. If you need that depth, read the memory files.

### Docs in the repo

- [`docs/architecture.md`](./architecture.md) — system diagram, wire
  protocol, deployment topology. The single source of truth for "how
  the pieces fit together".
- [`docs/release.md`](./release.md) — release / image-publish process.
- [`docs/provisioning/`](./provisioning/) — runbooks for the bits that
  live outside the repo:
  - `01-windows-test-vm.md` — building a Windows endpoint for agent
    testing
  - `02-entra-app-registration.md` — operator OAuth app registration
  - `03-intune-cloud-pki.md` — Wave 3 mTLS prerequisite

### Docs in the parser repo (submodule)

- `references/platform/` (inside `cmtraceopen/`) — phase specs that
  predate the platform split. Useful when you're trying to figure out
  the original design intent for a parser feature.

---

## Where to ask

The project is solo today. There's no Slack, no Discord, no shared
issue tracker beyond GitHub Issues on `cmtraceopen-web`. When that
changes — first additional contributor, first external user, first
Slack channel — update this section with the actual links. Until then:
file an issue on the repo, or page Adam directly if you have his
contact already.

---

_Last updated: 2026-04-21. If you find a gotcha that bit you and isn't
in this list, add it — that's the whole point of this doc._
