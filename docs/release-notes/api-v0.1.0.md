# api-server v0.1.0

First published image of the cmtraceopen `api-server`. This release marks the
walking-skeleton complete: a Windows agent can authenticate with a
PKCS-issued client cert, upload a chunked log bundle, the server parses
it on ingest, and an Entra-authenticated operator can browse devices,
sessions, and entries through the React viewer. End-to-end, no manual
glue.

## Overview

This is the first GHCR-published build of `api-server`. Everything below
has been exercised against the dev compose stack and the BigMac26 deploy
host. Treat it as a usable preview rather than a production GA — see
[Known limitations](#known-limitations) for the rough edges.

## Image

- Registry: `ghcr.io/adamgell/cmtraceopen-api:0.1.0`
- Also tagged: `latest`
- Architectures: `linux/amd64`, `linux/arm64` (single multi-arch manifest)
- Base image: `gcr.io/distroless/cc-debian12` (~40 MB final image, no
  shell, runs as UID/GID `65532:65532`)
- Built from: tag `api-v0.1.0`

## What's in this release

### Ingest protocol (chunked, resumable)

- `POST /v1/ingest/init`, `PUT /v1/ingest/{upload_id}/chunks/{n}`,
  `POST /v1/ingest/{upload_id}/finalize`
- Atomic per-chunk offset advance via conditional `UPDATE` (no
  double-write races on retries)
- 32 MiB body cap on ingest routes
- Resume rejects when init invariants drift (size, hash, device)
- Correlation fields on every span

### Parse-on-ingest pipeline

- Background worker drains finalized bundles through the
  `cmtraceopen-parser` (vendored as a submodule) and persists parsed
  entries into SQLite
- Handles CMTrace, CBS, Panther, and plain text fixtures (covered by
  the wasm-canary CI matrix)

### Query routes

- `GET /v1/devices`, `GET /v1/devices/{id}/sessions`,
  `GET /v1/sessions/{id}/entries` with keyset pagination
- Server-side filter pushdown for the viewer's FilterBar
- All query routes gated on the `Operator` Entra app role

### Storage

- `MetaStore` trait with a SQLite implementation (WAL + busy_timeout
  enabled, pool stats surfaced on the status page)
- `BlobStore` trait with a local-filesystem implementation
  (`<CMTRACE_DATA_DIR>/blobs`)
- Migrations baked into the binary

### Auth

- **Devices (ingest)**: mTLS termination via rustls, identity derived
  from the SAN URI in the client cert
  (`device://{tenant}/{aad-device-id}`). Trust anchors loaded from a
  PEM bundle that chains to the Gell CDW Workspace Labs Root + Issuing
  CAs. Falls back to legacy `X-Device-Id` header when
  `CMTRACE_MTLS_REQUIRE_INGEST=false` (transitional only).
- **Operators (queries)**: Entra ID JWT bearer tokens, JWKS cached
  in-process for 1 h, `aud`/`iss` validated. RBAC via the `Operator`
  and `Admin` app roles.
- `CMTRACE_AUTH_MODE=disabled` short-circuits the operator extractor
  with a synthetic principal — dev-only, the compose default.

### Other

- CORS layer with explicit allow-list (fail-closed default)
- `GET /` status page with request counter, sqlx pool stats, and
  startup config summary
- `GET /healthz`, `GET /readyz` for deploy probes
- `Admin` stub route surface (placeholder for future tenant ops)

## Configuration

All env vars use the `CMTRACE_` prefix. The compose file at
`docker-compose.yml` has a working dev set; values below are what
operators most commonly tune.

### Networking

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_LISTEN_ADDR` | `0.0.0.0:8080` | Bind address |
| `CMTRACE_CORS_ORIGINS` | empty (deny all cross-origin) | Comma-separated allow-list, e.g. `https://viewer.example.com` |
| `CMTRACE_CORS_CREDENTIALS` | `false` | Allow cookie/`Authorization` on cross-origin requests |

### Operator auth (Entra)

| Variable | Required when | Purpose |
| --- | --- | --- |
| `CMTRACE_AUTH_MODE` | always | `enabled` (prod) or `disabled` (local dev only) |
| `CMTRACE_ENTRA_TENANT_ID` | `auth_mode=enabled` | Tenant GUID |
| `CMTRACE_ENTRA_AUDIENCE` | `auth_mode=enabled` | Expected `aud` claim (your API app's app-id-uri or client-id) |
| `CMTRACE_ENTRA_JWKS_URI` | `auth_mode=enabled` | Tenant JWKS endpoint |

Partial Entra config is rejected at startup — set all three or none.

### TLS / mTLS

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_TLS_ENABLED` | `false` | Master switch for TLS termination |
| `CMTRACE_TLS_CERT` | — | Server cert PEM (required when enabled) |
| `CMTRACE_TLS_KEY` | — | Server key PEM (required when enabled) |
| `CMTRACE_CLIENT_CA_BUNDLE` | — | PEM bundle of trust anchors for client certs (required when enabled) |
| `CMTRACE_MTLS_REQUIRE_INGEST` | mirrors `CMTRACE_TLS_ENABLED` | If `false`, ingest accepts the legacy `X-Device-Id` header instead of a client cert |
| `CMTRACE_SAN_URI_SCHEME` | `device` | Expected URI scheme in the client-cert SAN |

### Storage

| Variable | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_DATA_DIR` | `./data` | Root for blob staging + finalized blobs |
| `CMTRACE_SQLITE_PATH` | `<data_dir>/meta.sqlite` | SQLite metadata DB (use `:memory:` for tests) |

### Observability

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUST_LOG` | `info` | Standard `tracing-subscriber` filter |

## Deployment

Minimal smoke run (anonymous mode, ephemeral data):

```sh
docker run --rm -p 8080:8080 \
  -e CMTRACE_AUTH_MODE=disabled \
  -e RUST_LOG=api_server=info,tower_http=info \
  ghcr.io/adamgell/cmtraceopen-api:0.1.0
```

Then hit `http://localhost:8080/` for the status page.

For a full dev stack (Postgres sidecar for future migration target +
Adminer at `:8082`), use the canonical `docker-compose.yml` at the repo
root:

```sh
git clone https://github.com/adamgell/cmtraceopen-web
cd cmtraceopen-web
docker compose up -d
```

Production deploys should mount cert/key/CA-bundle paths read-only,
set `CMTRACE_AUTH_MODE=enabled` with the full Entra triple, and
restrict `CMTRACE_CORS_ORIGINS` to the viewer's exact origin. The
release runbook at `docs/release.md` covers tag → publish → pull.

## Known limitations

- **aarch64-windows local dev**: `aws-lc-sys` (transitive via the
  rustls path) won't link locally on Windows-on-ARM. The CI image
  build is authoritative — develop natively on x86_64 or Linux/macOS,
  or rely on the published image.
- **CRL polling not wired**: the mTLS verifier checks chain + SAN but
  does not yet pull or honor CRLs. Revoking a device cert today
  requires removing the issuing-CA leaf from the bundle or restarting
  the server. Wave 3 follow-up.
- **Storage is local-FS only**: the `BlobStore` trait is in place but
  the only implementation in this build is `blob_fs`. No Azure Blob
  backend yet.
- **No `/metrics`**: status page exposes pool stats and a request
  counter, but there's no Prometheus exposition. Logs + tracing only.
- **Ingest fallback header is still wired**: `X-Device-Id` is accepted
  when `CMTRACE_MTLS_REQUIRE_INGEST=false`. Leave this off in any
  environment reachable from the public internet.
- **No CHANGELOG file yet**: the per-crate changelog called out in the
  release runbook hasn't been added — these notes are the source of
  truth for what shipped.

## What's next (Wave 3+)

- CRL polling + cache wired into the mTLS verifier
- `BlobStore` Azure Blob implementation (selectable via env)
- Prometheus `/metrics` endpoint
- Signed agent MSI, code-signed via the same PKI chain
- Per-crate `CHANGELOG.md` for `api-server`
- Ansible `compose_stack` switched to pull-by-tag for pinned deploys

## Upgrade notes

N/A — first published release.
