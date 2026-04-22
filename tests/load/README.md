# CMTrace Open — Load Tests

This directory contains [k6](https://k6.io/) load-test scenarios for the
`api-server`. They validate the capacity estimates documented in
[`docs/wave4/04-day2-operations.md §4`](../../docs/wave4/04-day2-operations.md)
and record actual results in
[`docs/wave4/17-capacity-results.md`](../../docs/wave4/17-capacity-results.md).

## Scenarios

| Script | Description |
|---|---|
| `k6-bundle-ingest.js` | 100 virtual devices × 1 bundle/minute × 5 minutes (init → chunk → finalize) |
| `k6-query-mix.js` | 10 virtual operators each issuing list-sessions + get-session + get-entries at ~1 req/sec |

---

## Prerequisites

### Install k6

```bash
# macOS (Homebrew)
brew install k6

# Linux (Debian/Ubuntu)
sudo apt-get update && sudo apt-get install -y k6

# Windows (Winget)
winget install k6 --source winget
```

See the [official install guide](https://grafana.com/docs/k6/latest/set-up/install-k6/)
for other platforms.

### Start the local stack

```bash
# From the repo root
docker compose up --build -d

# Verify the server is ready
curl -s http://localhost:8080/healthz
```

---

## Running locally

### Bundle-ingest scenario

```bash
BASE_URL=http://localhost:8080 k6 run tests/load/k6-bundle-ingest.js
```

Optional overrides:

| Variable | Default | Description |
|---|---|---|
| `BASE_URL` | `http://localhost:8080` | API server base URL |
| `DEVICES` | `100` | Number of concurrent virtual devices |
| `DURATION` | `5m` | Total test duration |
| `CHUNK_SIZE` | `4096` | Bytes per chunk payload |

Example (reduced scale for a quick smoke check):

```bash
BASE_URL=http://localhost:8080 DEVICES=10 DURATION=1m k6 run tests/load/k6-bundle-ingest.js
```

### Operator query-mix scenario

Run **after** the ingest scenario (or after some real data exists) so the
read endpoints have sessions to return.

```bash
BASE_URL=http://localhost:8080 k6 run tests/load/k6-query-mix.js
```

Optional overrides:

| Variable | Default | Description |
|---|---|---|
| `BASE_URL` | `http://localhost:8080` | API server base URL |
| `OPERATORS` | `10` | Number of concurrent virtual operators |
| `DURATION` | `5m` | Total test duration |
| `DEVICE_IDS` | `LOAD-DEVICE-0001,…,LOAD-DEVICE-0010` | Comma-separated device IDs to query |

---

## Running against BigMac26

Replace `BASE_URL` with the server's address. The `X-Device-Id` header
authentication mode must be active (`CMTRACE_AUTH_MODE=disabled` or
`CMTRACE_MTLS_REQUIRE_INGEST=false`) for the header-based ingest path to work:

```bash
BASE_URL=https://bigmac26.local:8443 \
  DEVICES=100 \
  DURATION=5m \
  k6 run tests/load/k6-bundle-ingest.js
```

If mTLS is enforced, run k6 from a host that has a valid client cert and
configure k6's TLS client certificate:

```bash
k6 run \
  --config /dev/null \
  -e BASE_URL=https://bigmac26.local:8443 \
  --insecure-skip-tls-verify \   # only if using self-signed CA
  tests/load/k6-bundle-ingest.js
```

> **Note:** k6's built-in client-cert support uses `--tls-cert` and `--tls-key`
> flags. Full mTLS setup is outside the scope of this README; see the k6 docs
> on [TLS](https://grafana.com/docs/k6/latest/using-k6/protocols/ssl-tls/).

---

## Interpreting results

k6 prints a summary table at the end of each run. Key metrics to capture for
[`docs/wave4/17-capacity-results.md`](../../docs/wave4/17-capacity-results.md):

### Ingest scenario

| Metric | Threshold | Notes |
|---|---|---|
| `bundle_finalize_duration` p50/p95/p99 | p95 < 2 s | Core latency SLO |
| `bundle_error_rate` | < 1 % | Any non-200 from init/chunk/finalize |
| `http_req_failed` | < 1 % | Network-level failures |

### Query scenario

| Metric | Threshold | Notes |
|---|---|---|
| `query_list_sessions_duration` p50/p95/p99 | p95 < 500 ms | Paginated list |
| `query_get_session_duration` p50/p95/p99 | p95 < 500 ms | Single session fetch |
| `query_get_entries_duration` p50/p95/p99 | p95 < 500 ms | Entry list |
| `query_error_rate` | < 1 % | Any non-200/404 response |

---

## Backends tested

Run each scenario against the backends listed in the issue to populate
`docs/wave4/17-capacity-results.md`:

1. **SQLite + local-FS blob** (default `docker compose up`)
2. **Postgres + local-FS blob** (set `CMTRACE_DATABASE_URL` and switch backend)
3. **SQLite + Azure Blob** (set `CMTRACE_BLOB_STORE=azure` + Azure connection string)
4. **Postgres + Azure Blob** (combination of above)

### Automated multi-backend run

`run-all-backends.sh` cycles through all four combinations automatically,
restarting the Compose stack between each run and saving timestamped output
files under `tests/load/results/`:

```bash
# From the repo root — runs both scenarios × 4 backends
bash tests/load/run-all-backends.sh

# Override duration / scale for a quick check
DURATION=1m DEVICES=10 bash tests/load/run-all-backends.sh

# Run against BigMac26
BASE_URL=http://bigmac26.local:8080 bash tests/load/run-all-backends.sh
```

> **Note:** The Azure Blob backend requires `AZURE_STORAGE_CONNECTION_STRING`
> (or equivalent) to be set in the shell before running the script; the
> Compose stack passes it through to the api-server container automatically.

---

## HTTP protocol note

k6 negotiates HTTP/2 by default when the server advertises ALPN `h2`. The
api-server (axum/hyper) currently only advertises HTTP/1.1 unless TLS
termination is in front of it, so in practice all connections use HTTP/1.1
with head-of-line blocking. Record the actual protocol negotiated in
`docs/wave4/17-capacity-results.md §6` — it affects p99 latency materially
when 100 VUs share a connection pool. To force HTTP/1.1 explicitly, pass
`--http-debug` to k6 and confirm `h2` never appears in the request log.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `ECONNREFUSED` on all requests | Server not running | `docker compose up --build` |
| `401 Unauthorized` on init | Auth mode not disabled | Set `CMTRACE_AUTH_MODE=disabled` in compose |
| `400 Bad Request` on init | Payload field-name mismatch | Confirm you are running the latest script; fields must be camelCase (`bundleId`, `sizeBytes`, `contentKind`) |
| `400 Bad Request` on finalize with `Sha256Mismatch` | Script using fake hash | Confirm you are running the latest script; it uses `k6/crypto.sha256()` for a real digest |
| High `bundle_error_rate` with 413 | Chunk size too large | Reduce `CHUNK_SIZE` (default 4096 is well under limit) |
| `database is locked` in server logs | SQLite write contention | Expected under high concurrency; document in capacity results |
