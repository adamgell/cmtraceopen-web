# 19 — Rate Limiting (DoS protection)

Per-device and per-IP rate limits added in the `api-server` to protect the
ingest + query surfaces against misbehaving agents, compromised devices, and
opportunistic flooding before the Azure AppGW WAF provides a second defence
layer (see [`docs/wave4/05-azure-deploy.md`](05-azure-deploy.md)).

---

## Design

The implementation uses a **fixed-window counter** backed by a
[`DashMap`](https://docs.rs/dashmap) (already in the dependency tree) so
no new crates are added and the per-key locking is lock-free across different
keys. Three independent scopes are enforced via Axum middleware layers wired
onto two route groups:

| Scope          | Route group                             | Default limit     | Window |
|----------------|-----------------------------------------|-------------------|--------|
| `device`       | `/v1/ingest/*`                          | 100 req / device  | 1 hour |
| `ip` (ingest)  | `/v1/ingest/*`                          | 1 000 req / IP    | 1 min  |
| `ip` (query)   | `/v1/devices`, sessions, files, entries | 60 req / IP       | 1 min  |

Setting any limit to `0` disables that scope entirely.

### Why fixed-window?

A sliding-window counter would be slightly more accurate at boundaries but
adds complexity with no practical benefit at these thresholds. The 8-device
beta pilot generates well under 10 ingest calls per device per hour; the
default 100/h leaves an order-of-magnitude margin.

### Memory management

Two lines of defence keep the limiter from becoming a DoS vector against
itself:

1. **Background GC (steady state).** A Tokio task calls
   `RateLimiter::purge_expired()` once per minute, evicting entries
   whose window has fully elapsed. Bounds the map footprint to the
   number of distinct keys seen within **one** window in normal traffic.
2. **In-line hard cap (burst attack).** Each limiter is hard-capped at
   `RATE_LIMIT_MAX_KEYS = 50_000` distinct keys. When the cap is
   reached the hot path runs an opportunistic sweep before inserting a
   new key. If the sweep can't reclaim a slot (e.g. an attacker churns
   50 000 IPv6 addresses inside a single minute), new keys are admitted
   without being inserted (the limiter fails open on cap exhaustion —
   cap-exhaustion lockout would itself be a DoS). Existing keys remain
   enforced regardless of map size.

#### IPv6 attacker rotation (known limitation)

Even with the hard cap and per-minute GC, an IPv6 attacker can churn
keys faster than either layer absorbs (a single /64 prefix gives ~1.8e19
addresses). Once map capacity is reached the limiter is effectively
disabled for new IPv6 sources from the attacker. A future refinement is
to **collapse IPv6 keys onto their /64 prefix** so per-IPv6-customer-
allocation rather than per-address counting — this would make the
limiter robust against single-customer floods. Tracked as a follow-up;
the AppGW WAF + Azure DDoS Protection upstream is the load-bearing
IPv6-flood mitigation today.

### IP identification

Source IP is derived as follows (in order):

1. **`ConnectInfo<SocketAddr>`** (TCP peer address) — always present in
   production because `main.rs` uses
   `into_make_service_with_connect_info::<SocketAddr>()`.
   - If the peer is **not** in `CMTRACE_TRUSTED_PROXY_CIDRS` → use peer IP
     directly. Forwarded headers are ignored and cannot be spoofed.
   - If the peer **is** in a trusted CIDR → read the first hop from
     `X-Forwarded-For`, then `X-Real-Ip`.
2. **Header fallback** — only when `ConnectInfo` is absent (integration-test
   path using plain `axum::serve`). Tests simulate different IPs via
   `X-Forwarded-For`.

Set `CMTRACE_TRUSTED_PROXY_CIDRS` to the Azure Application Gateway frontend
subnet CIDR(s) so the limiter counts the real client IP forwarded by the
AppGW WAF instead of the AppGW's own address.

When trusted proxy CIDRs are empty (the default), **no forwarded header is
ever honoured** — an attacker cannot bypass per-IP limits by setting
`X-Forwarded-For`.

### Device identity

The device limiter key is resolved in the same priority order as the
`DeviceIdentity` extractor:

1. A `DeviceIdentity` already stashed in request extensions by a prior layer.
2. mTLS peer-cert SAN URI (`PeerCertChain` extension, `mtls` feature).
3. `X-Device-Id` header (legacy transitional path).
4. `"__unknown__"` sentinel.

---

## Configuration

All variables use the `CMTRACE_` prefix:

| Variable                                      | Default | Description                                            |
|-----------------------------------------------|---------|--------------------------------------------------------|
| `CMTRACE_RATE_LIMIT_INGEST_PER_DEVICE_HOUR`   | `100`   | Bundle ingest calls per device ID per hour. `0` = off. |
| `CMTRACE_RATE_LIMIT_INGEST_PER_IP_MINUTE`     | `1000`  | Ingest requests per source IP per minute. `0` = off.  |
| `CMTRACE_RATE_LIMIT_QUERY_PER_IP_MINUTE`      | `60`    | Query requests per source IP per minute. `0` = off.   |
| `CMTRACE_TRUSTED_PROXY_CIDRS`                 | `""`    | Comma-separated CIDRs that may forward `X-Forwarded-For`. Empty = honour no forwarded headers (safest). |

### Disabling all limits (dev-only)

```bash
CMTRACE_RATE_LIMIT_INGEST_PER_DEVICE_HOUR=0 \
CMTRACE_RATE_LIMIT_INGEST_PER_IP_MINUTE=0 \
CMTRACE_RATE_LIMIT_QUERY_PER_IP_MINUTE=0 \
cargo run -p api-server
```

### Production (AppGW in front)

```bash
# AppGW frontend subnet — only XFF from this range is trusted
CMTRACE_TRUSTED_PROXY_CIDRS=10.0.1.0/24
```

The AppGW WAF rule must **overwrite** (not append) `X-Forwarded-For` with
the real client IP. If the rule only appends, an attacker can inject a
spoofed first hop before the AppGW's value.

---

## 429 Response

When a limit is exceeded the server returns:

```
HTTP/1.1 429 Too Many Requests
Content-Type: application/json
Retry-After: 60
```

```json
{
  "error": "rate_limit_exceeded",
  "message": "[device] ingest rate limit exceeded for this device; check Retry-After and reduce upload frequency"
}
```

The `[device]` or `[ip]` prefix in `message` identifies which scope fired
without leaking other devices' counts or bucket states.

---

## Metrics

Every rejected request increments:

```
cmtrace_rate_limit_rejected_total{scope="device|ip", route="<matched-path>"}
```

Prometheus + Grafana alert rule (add to your alert rules config):

```yaml
- alert: RateLimitHigh
  expr: rate(cmtrace_rate_limit_rejected_total[5m]) > 10
  for: 1m
  labels:
    severity: warning
  annotations:
    summary: "Rate limit rejections elevated ({{ $labels.scope }}/{{ $labels.route }})"
```

---

## Files changed

| File                                                   | Change                                |
|--------------------------------------------------------|---------------------------------------|
| `crates/api-server/Cargo.toml`                         | Add `ipnet = "2"` as explicit dep     |
| `crates/api-server/src/config.rs`                      | `RateLimitConfig` + `trusted_proxy_cidrs` field + env-var parsing |
| `crates/api-server/src/state.rs`                       | `RateLimiter`, `RateLimitState`, `purge_expired()`, `len()` |
| `crates/api-server/src/middleware/mod.rs`              | NEW — middleware module declaration   |
| `crates/api-server/src/middleware/rate_limit.rs`       | NEW — three middleware functions; CIDR-aware `extract_ip`; cert-priority `resolve_device_id` |
| `crates/api-server/src/auth/device_identity.rs`        | Add `extract_device_id_from_leaf` (pub crate, mtls feature) |
| `crates/api-server/src/lib.rs`                         | Wire middleware on ingest + query sub-routers |
| `crates/api-server/src/main.rs`                        | GC background task; `into_make_service_with_connect_info`; log trusted CIDRs |
| `crates/api-server/tests/rate_limit_integration.rs`    | NEW — 7 integration tests             |
| `docs/wave4/19-rate-limiting.md`                       | Design, config, 429 shape, metrics, acceptance criteria |

---

## Acceptance criteria checklist

- [x] Single device exceeding limit gets 429s; other devices unaffected
- [x] Limits configurable via env vars; setting to 0 disables
- [x] 429 response includes `Retry-After` header
- [x] Operator query routes have separate limits from agent ingest
- [x] Default limits leave headroom for the 8-device pilot (100 req/device/h vs ~10 real)
- [x] DashMap footprint bounded by `purge_expired()` GC task (once/min)
- [x] IP source secured: peer IP used directly unless peer CIDR is in `CMTRACE_TRUSTED_PROXY_CIDRS`
- [x] Device key resolved from cert SAN URI before falling back to header

---

## Future work (out of scope for this PR)

- **Distributed rate limiting** — a Redis-backed store (e.g. `governor`'s
  `RedisStore` or a custom `redis` + `deadpool` adapter) replaces the
  in-process `DashMap` when the api-server goes HA (multi-replica).
- **Burst credit** — the current fixed-window model doesn't allow short
  burst credit; `tower_governor`'s leaky-bucket or a token-bucket counter
  can be slotted in as a drop-in replacement for `RateLimiter::check`.
- **AppGW WAF rules** — the Azure Application Gateway WAF provides a
  defence-in-depth layer on top; see
  [`docs/wave4/05-azure-deploy.md`](05-azure-deploy.md).