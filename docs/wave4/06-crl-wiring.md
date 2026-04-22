# Wave 4 — Wiring CRL Revocation into the mTLS Reject Path

## Problem

PR #47 landed `CrlCache` with a background refresh loop that polls the Cloud
PKI Root + Issuing CRLs hourly and exposes `pub fn is_revoked(&self, serial:
&[u8]) -> bool`. The cache is plumbed onto `AppState::crl_cache` (gated
`#[cfg(feature = "crl")]`), but **nothing in the request path consults it**.
A revoked agent cert today still authenticates as long as it hasn't expired.

This change is the ~30-line plumbing job that makes PR #47 actually do
something: consult the cache inside the `DeviceIdentity` extractor and reject
revoked certs before the handler runs.

## Goal

When an agent presents a client cert, before constructing the `DeviceIdentity`
value:

1. Pull the leaf cert's serial bytes (`tbs_certificate.raw_serial()` from
   `x509-parser`).
2. Hand them to `crl_cache.revocation_status(serial)`.
3. Decide based on the matrix below; emit a structured log + metric on every
   non-trivial outcome.

## Where the check lives

**Inside `DeviceIdentity::from_request_parts`**, immediately after the
peer-cert is pulled out of `parts.extensions` and before the existing SAN-URI
parse. That's the latest point at which we still hold the leaf DER and the
earliest point at which we have an authenticated cert worth checking.

The extractor is the right place — not the handshake — because:

- The TLS-handshake-time alternative (`WebPkiClientVerifier::with_crls(...)`
  on rustls 0.23) requires a different feature combo and would force a
  listener re-bind on every CRL refresh. Already covered in the
  `crl.rs` module docstring; repeated here for the next reader.
- Extractor-time check picks up CRL refreshes for free — the cache swaps the
  inner map atomically and the next request consults the new map.
- `--no-default-features` builds (no `crl` feature) collapse the call to a
  no-op accept, matching today's posture exactly.

## Decision matrix

`CrlCache::revocation_status(serial)` returns a new `RevocationStatus`
three-valued enum so the extractor can distinguish "explicitly revoked" from
"unknown (cache empty)" and apply `crl_fail_open` at the call site rather
than burying it inside `is_revoked`'s return-bool. The matrix:

| `revocation_status` | `crl_fail_open` | Action                                                                                               |
|---|---|---|
| `Revoked`            | (any)  | reject **401** + `WWW-Authenticate: cert-revoked`, `tracing::warn!`, `cmtrace_crl_revocations_total{result="rejected"}` |
| `NotRevoked`         | (any)  | accept; no metric                                                                                    |
| `Unknown`            | `true` | accept; `cmtrace_crl_revocations_total{result="unknown_fail_open"}`                                  |
| `Unknown`            | `false`| reject **503** + `Retry-After: 60`, `tracing::warn!`, `cmtrace_crl_revocations_total{result="unknown_fail_closed"}` |

`Unknown` covers two real situations:

- the cache has never landed a successful fetch (cold-start network blip), or
- the cache has fetches but none of them list this cert's serial **and** none
  of them are the issuing CA's CRL (e.g. only the Root CRL has been fetched
  so far). The current implementation flattens this to "no CRL has the
  serial" — good enough; the failure mode is identical from the caller's
  perspective.

503 (not 401) on `Unknown + fail-closed` because the request *might* succeed
on retry once a CRL fetch lands, and `Retry-After: 60` matches the default
1-hour refresh interval's worst-case patience without flooding the listener.

## Serial-encoding gotcha

RFC 5280 §4.1.2.2 says cert serials are positive `INTEGER` up to 20 octets,
which means a leading `0x00` byte is REQUIRED if the high bit of the first
real byte would otherwise be set (otherwise the integer would parse as
negative). Both sides of the comparison use `x509-parser`'s `raw_serial()`,
which returns the bytes as the CA wrote them — INCLUDING the disambiguation
zero — so byte-for-byte comparison is correct.

`crl.rs::CrlCache::parse` already consumes `entry.raw_serial().to_vec()` (see
the comment around line 282 — "we keep the padding so equality with the leaf
serial (also from `raw_serial`) is exact"), and the extractor uses the same
`raw_serial()` accessor on the leaf side. No normalization is needed.

## Cargo feature interaction

The new check is gated `#[cfg(feature = "crl")]`. With `--no-default-features`
(or with `--no-default-features --features mtls`) the block compiles out and
the extractor behaves exactly as today's `main`: identity is established
purely from the SAN URI with no revocation lookup.

## New metric

`cmtrace_crl_revocations_total` (Counter) with one label `result` taking
three values:

- `rejected` — explicit revocation hit. Operator-visible alert candidate
  (sustained non-zero rate suggests the device fleet has stale credentials).
- `unknown_fail_open` — request let through under `crl_fail_open=true` despite
  an empty/cold cache. In production this should sit at zero after the first
  successful fetch; sustained non-zero means the refresh task is stuck.
- `unknown_fail_closed` — request rejected under `crl_fail_open=false`
  because the cache hasn't landed any CRL yet. Should be a startup-only
  bump, then zero.

Description string registered in `main.rs::describe_metrics()` so it shows up
on `/metrics` alongside the rest.

## Test plan

Three new unit tests in `crates/api-server/src/auth/device_identity.rs`,
co-located with the existing extractor tests so they share the same
`AppState` builder helpers:

- **`crl_revoked_serial_returns_401`** — install a `CrlCache` containing one
  CRL with serial `[0x42]`, build a request with `PeerCertChain` carrying a
  leaf cert with serial `0x42`, run the extractor, assert `401 Unauthorized`
  with `WWW-Authenticate: cert-revoked`.
- **`crl_unknown_serial_fail_open_passes`** — empty `CrlCache` constructed
  with `fail_open=true`, build a request with PeerCertChain whose leaf has
  serial `0x99` (not in any CRL), assert the extractor returns `Ok` and the
  identity is established normally.
- **`crl_unknown_serial_fail_closed_returns_503`** — empty `CrlCache` with
  `fail_open=false`, same setup as above, assert `503 Service Unavailable`
  with `Retry-After: 60`.

Each test mints a tiny self-signed cert via `rcgen` (already a dev-dep
behind the `test-mtls` feature). The first test uses the cert's actual
serial as the cache entry; rcgen lets us pick the serial deterministically.

(In the implementation we instead reach into `CrlCache::insert_for_test` and
hand-craft a leaf via the same `build_minimal_crl` helper layout so the
tests don't introduce an `rcgen` dependency on the non-`test-mtls` feature
build. The choice is mechanical — see the test source for which path was
taken.)

## Open questions

None material. All design decisions inherited from PR #47:

- Fail-open vs fail-closed default → already `false` (closed) per `Config`.
- Refresh cadence → already 1 hour per `crl_refresh_secs`.
- CDN URLs → already documented in `Config::crl_urls` rustdoc and
  `reference_cloud_pki.md`.
