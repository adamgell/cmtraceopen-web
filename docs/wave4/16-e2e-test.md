# Wave 4 End-to-End Test

## Purpose

Single end-to-end test (`crates/api-server/tests/wave4_e2e.rs`) that
exercises the full Wave 4 stack in a single test run.  Catches drift between
the mTLS, RBAC, ingest, parse, and query layers before a regression reaches
production.

## What It Covers

| Layer | Assertion |
|---|---|
| **mTLS cert path** | Ingest with device leaf cert uses cert-derived identity (SAN URI parsed → `device_id`) |
| **mTLS extractor rejection** (`require_on_ingest=false`) | Ingest without cert + without `X-Device-Id` header → 401 from the application-layer `DeviceIdentity` extractor |
| **mTLS handshake rejection** (`require_on_ingest=true`) | A bare TLS client without a client cert is blocked: either the rustls handshake itself rejects (connect() fails) or — due to TLS 1.3 post-handshake cert verification timing — the subsequent HTTP request returns 401 from the `DeviceIdentity` extractor. Both outcomes prevent ingest. Asserted in `wave4_mtls_handshake_rejects_unauthenticated_client`. |
| **JWT validation** | Query routes require a valid Entra RS256 bearer token |
| **JWT rejection** | Query routes without a bearer token → 401 |
| **RBAC happy-path** | Token carrying `scp=CmtraceOpen.Query` gains access to device/session/entries |
| **RBAC negative-path** | Token without `CmtraceOpen.Query` (valid signature + audience but missing scope) → **401** (`InsufficientScope`) from query routes. Re-authentication with the correct scope is required; this is distinct from 403 (`ForbiddenRole`) which fires when a principal *has* a valid role but accesses a route that requires a higher role. Asserted in `wave4_query_rejects_token_without_query_scope`. |
| **Ingest pipeline** | Full init → chunk → finalize flow with an evidence-zip payload |
| **Parse worker** | Background parse flips `parse_state` to `ok` and populates entries |
| **Query layer** | Device appears in registry; session tied to that device; entries with correct severity distribution |

### Severity sanity-check

The evidence-zip shipped in the test contains three CMTrace-format log lines
(types 1 / 2 / 3), which the parse worker maps to:

| CMTrace type | Expected `severity` |
|---|---|
| 1 | `"Info"` |
| 2 | `"Warning"` |
| 3 | `"Error"` |

The test asserts that all three are present in the queried entries.

## What It Does NOT Cover

- Real Cloud PKI certs (the synthesized cert proves the code path)
- CRL revocation (separate test under the `crl` feature)
- Multi-tenant isolation
- Token expiry and JWKS refresh

## How to Run

```bash
cargo test -p api-server --features test-mtls wave4_e2e
```

The `test-mtls` feature implies `mtls`, which pulls in `axum-server` /
`rustls` / `aws-lc-sys`.  These crates require **cmake** and (on Windows)
**NASM** to compile `aws-lc-sys`.  They are intentionally NOT included in
the default feature set so plain `cargo test` on dev boxes without these
tools keeps working.

CI (`ci.yml`) explicitly passes `--features test-mtls` on the api-server
test step.

## State Isolation

Each test run:
- Creates a fresh `TempDir` for blob storage (leaked after server start;
  the OS reclaims it on process exit).
- Uses an in-memory SQLite database (`:memory:`).
- Mints a unique self-signed CA + leaf certs — no file system state is
  shared between parallel test runs.

## Test Architecture

```
mint_pki()
  └─ self-signed root CA
     ├─ server leaf cert (SAN=localhost, EKU=serverAuth)
     └─ client leaf cert (SAN URI=device://<tenant>/<device>, EKU=clientAuth)

start_wave4_server()
  ├─ AppState (auth=Enabled, mtls.require_on_ingest=false)
  ├─ serve_tls_with_handle()  ← axum-server with PeerCertCapturingAcceptor
  ├─ mTLS TlsConnector        ← presents client leaf cert
  ├─ plain TlsConnector       ← no client cert (for rejection test)
  └─ reqwest::Client          ← HTTPS, no client cert (for JWT query routes)

wave4_e2e_mtls_ingest_and_jwt_query()
  1. Rejection: plain connector + no header → 401
  2. Init (mTLS)
  3. Chunk (mTLS)
  4. Finalize (mTLS) → parse_state="pending"
  5. GET /v1/devices (JWT) → device visible
  6. GET /v1/devices/{id}/sessions (JWT) → session visible
  7. Poll GET /v1/sessions/{id} until parse_state="ok"
  8. GET /v1/sessions/{id}/entries (JWT) → Info+Warning+Error present
  9. GET /v1/devices (no JWT) → 401
```

## Why `require_on_ingest = false`

Setting `require_on_ingest = true` makes the rustls `ServerConfig` use a
verifier that does **not** call `allow_unauthenticated()`.  This means
**every** TLS connection — including the JWT-only reqwest client hitting the
query routes — must present a client cert at the TLS handshake level.  That
would require wiring a client cert into reqwest and adds complexity with no
benefit.

With `require_on_ingest = false` the TLS-level verifier uses
`allow_unauthenticated()`, so the reqwest client can connect without a cert.
The mTLS cert path is still fully exercised: the tokio-rustls ingest client
presents the leaf cert, the `DeviceIdentity` extractor parses its SAN URI,
and the rejection path is exercised by the plain connector sub-assertion.

## Cross-references

- `crates/api-server/tests/mtls_integration.rs` — the earlier unit-level
  mTLS test this e2e complements
- `docs/wave4/03-beta-pilot-runbook.md` — beta success metric depends on
  this stack working end-to-end
- `docs/provisioning/03-intune-cloud-pki.md` — describes the real Cloud PKI
  cert profile the synthesized cert mimics
