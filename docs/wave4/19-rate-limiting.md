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

### IP identification

Source IP is read from the `X-Forwarded-For` header (first hop), then
`X-Real-Ip`, then falls back to the sentinel key `__unknown__`. In
production the Azure Application Gateway must be configured to **overwrite**
`X-Forwarded-For` so clients cannot spoof it. Unrecognised IPs all share
the `__unknown__` bucket, which means the fallback bucket exhausts at the
configured threshold — a safe default for unproxied test traffic.

---

## Configuration

All variables use the `CMTRACE_` prefix:

| Variable                                      | Default | Description                                            |
|-----------------------------------------------|---------|--------------------------------------------------------|
| `CMTRACE_RATE_LIMIT_INGEST_PER_DEVICE_HOUR`   | `100`   | Bundle ingest calls per device ID per hour. `0` = off. |
| `CMTRACE_RATE_LIMIT_INGEST_PER_IP_MINUTE`     | `1000`  | Ingest requests per source IP per minute. `0` = off.  |
| `CMTRACE_RATE_LIMIT_QUERY_PER_IP_MINUTE`      | `60`    | Query requests per source IP per minute. `0` = off.   |

### Disabling all limits (dev-only)

```bash
CMTRACE_RATE_LIMIT_INGEST_PER_DEVICE_HOUR=0 \
CMTRACE_RATE_LIMIT_INGEST_PER_IP_MINUTE=0 \
CMTRACE_RATE_LIMIT_QUERY_PER_IP_MINUTE=0 \
cargo run -p api-server
```

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
| `crates/api-server/src/config.rs`                      | `RateLimitConfig` + env-var parsing   |
| `crates/api-server/src/state.rs`                       | `RateLimiter`, `RateLimitState`, `AppState::rate_limit` field |
| `crates/api-server/src/middleware/mod.rs`              | NEW — middleware module declaration   |
| `crates/api-server/src/middleware/rate_limit.rs`       | NEW — three middleware functions      |
| `crates/api-server/src/lib.rs`                         | Wire middleware on ingest + query sub-routers |
| `crates/api-server/src/main.rs`                        | Pass `RateLimitState` to `AppState`   |
| `crates/api-server/tests/rate_limit_integration.rs`    | NEW — 7 integration tests             |

---

## Acceptance criteria checklist

- [x] Single device exceeding limit gets 429s; other devices unaffected
- [x] Limits configurable via env vars; setting to 0 disables
- [x] 429 response includes `Retry-After` header
- [x] Operator query routes have separate limits from agent ingest
- [x] Default limits leave headroom for the 8-device pilot (100 req/device/h vs ~10 real)

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
