# Wave 4 — Capacity Results

> **Status:** Pre-beta load test — results to be filled in before first
> production deployment.
>
> Run both k6 scenarios (see [`tests/load/README.md`](../../tests/load/README.md))
> against every backend listed below and record the numbers in the tables.
> Compare against the estimates in
> [`docs/wave4/04-day2-operations.md §4`](04-day2-operations.md).

---

## 1. Test configuration

| Parameter | Value |
|---|---|
| **k6 version** | _fill in_ |
| **api-server commit** | _fill in_ |
| **Test host** | _fill in (e.g., BigMac26 / dev laptop / CI runner)_ |
| **Run date** | _fill in_ |
| **Ingest scenario VUs** | 100 devices × 1 bundle/min × 5 min |
| **Query scenario VUs** | 10 operators × ~1 req/sec × 5 min |
| **Chunk size** | 4096 bytes (default) |

---

## 2. Bundle-ingest results (`k6-bundle-ingest.js`)

### 2.1 SQLite + local-FS blob (default compose stack)

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `bundle_init_duration` (ms) | — | — | — | |
| `bundle_chunk_duration` (ms) | — | — | — | |
| `bundle_finalize_duration` (ms) | — | — | — | |
| `bundle_error_rate` | — | | | |
| `http_req_failed` | — | | | |

Server observations:
- `database is locked` warnings: _none / N occurrences_
- SQLite pool saturation (`pool_size` on status page): _fill in_
- Peak RSS of api-server container: _fill in_

### 2.2 Postgres + local-FS blob

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `bundle_init_duration` (ms) | — | — | — | |
| `bundle_chunk_duration` (ms) | — | — | — | |
| `bundle_finalize_duration` (ms) | — | — | — | |
| `bundle_error_rate` | — | | | |
| `http_req_failed` | — | | | |

### 2.3 SQLite + Azure Blob

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `bundle_init_duration` (ms) | — | — | — | |
| `bundle_chunk_duration` (ms) | — | — | — | |
| `bundle_finalize_duration` (ms) | — | — | — | |
| `bundle_error_rate` | — | | | |
| `http_req_failed` | — | | | |

Azure-specific observations:
- Throttling / quota errors: _none / describe_
- Follow-up issues opened: _link or "none"_

### 2.4 Postgres + Azure Blob

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `bundle_init_duration` (ms) | — | — | — | |
| `bundle_chunk_duration` (ms) | — | — | — | |
| `bundle_finalize_duration` (ms) | — | — | — | |
| `bundle_error_rate` | — | | | |
| `http_req_failed` | — | | | |

---

## 3. Operator query-mix results (`k6-query-mix.js`)

> Pre-requisite: run ingest scenario first so there is data to read.

### 3.1 SQLite + local-FS blob

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `query_list_sessions_duration` (ms) | — | — | — | |
| `query_get_session_duration` (ms) | — | — | — | |
| `query_get_entries_duration` (ms) | — | — | — | |
| `query_error_rate` | — | | | |

### 3.2 Postgres + local-FS blob

| Metric | p50 | p95 | p99 | Notes |
|---|---|---|---|---|
| `query_list_sessions_duration` (ms) | — | — | — | |
| `query_get_session_duration` (ms) | — | — | — | |
| `query_get_entries_duration` (ms) | — | — | — | |
| `query_error_rate` | — | | | |

---

## 4. Comparison with day-2 operations estimates

[`docs/wave4/04-day2-operations.md §4`](04-day2-operations.md) states:

> | Tier | PoC ceiling |
> |---|---|
> | Devices | ~100 (SQLite comfortable) |
> | Bundles / day | ~400 / day (100 devices × 4 bundles) — SQLite fine |
> | SQLite cap | > 250 devices → migrate to Postgres |

| Estimate | Source doc | Actual result | Confirmed? |
|---|---|---|---|
| 100 devices × 4 bundles/day is "trivial" for SQLite | §4.1 | _fill in_ | ✅ / ❌ / ⚠️ |
| SQLite caps at ~250 devices | §4.1 | _not tested — stress test deferred_ | — |
| Finalize latency acceptable at 100 VUs | §4.1 | p95 = _fill in_ ms | ✅ / ❌ |
| No `database is locked` under PoC load | §4.2 | _fill in_ | ✅ / ❌ |

Corrections or additions to `04-day2-operations.md` needed:

- _none_ (update this section after filling in the actuals above)

---

## 5. Follow-up issues

Any failure or surprising result from the load test that needs a separate
investigation:

| # | Backend | Symptom | Severity | Issue |
|---|---|---|---|---|
| — | — | — | — | — |

> Update this table with GitHub issue links as follow-ups are opened.

---

## 6. Raw k6 output

Paste or attach the full `k6 run` terminal output for each scenario/backend
combination here (or link to CI artifacts if run in a pipeline):

```
# Example placeholder — replace with actual output
# k6-bundle-ingest.js / SQLite + local-FS blob

          /\      |‾‾| /‾‾/   /‾‾/
     /\  /  \     |  |/  /   /  /
    /  \/    \    |     (   /   ‾‾\
   /          \   |  |\  \ |  (‾)  |
  / __________ \  |__| \__\ \_____/ .io

  execution: local
     script: tests/load/k6-bundle-ingest.js
     output: -

  scenarios: (100.00%) 1 scenario, 100 max VUs, 5m30s max duration (incl. graceful stop):
           * bundle_ingest: 100 looping VUs for 5m0s (gracefulStop: 30s)

<results will appear here>
```
