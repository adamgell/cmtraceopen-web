# Parse Load Harness — Design

**Status:** approved 2026-04-29
**Owner:** Adam
**Target crate:** `crates/parse-load-harness/`
**Companion:** `crates/parse-load-harness/bin/mine-dump.rs`

## Problem

The api-server's parse pipeline just landed three concurrency improvements (semaphore-bounded fan-out, in-bundle parallel parse, parse↔INSERT pipelining). Production validation on the pilot env exposed real-world stress shapes — 5 devices generating 4.4M entries, 201-deep parse backlog, OOM at 1Gi — but the pilot is now offline pending a rebuild and the next round of throughput work needs a *repeatable*, *reproducible*, *local-or-remote* way to push the system into and past those regimes.

We need a load harness that:
- Runs against three targets (in-process, local Docker stack, deployed pilot) from one binary.
- Drives benchmark, breaking-point, and soak load shapes from one event loop.
- Generates realistic bundles synthetically (informed by the pilot dump's distributions) and replays real bundles when available.
- Streams structured per-bundle and system-metrics output to disk so we can correlate spikes after the fact and compare two runs to detect regressions.

The harness is not a one-off script. It's first-class subsystem tooling that should drive improvements across the codebase: parser hot-loop optimization, DB query tuning, blob backend selection, mTLS overhead measurement, semaphore tuning, and (eventually) a CI regression gate.

## Goals

- **One binary, three targets** — `--target=in-process|local-http|remote-http`.
- **One driver, three load shapes** — concurrency / ramp / soak / total-bundles flags compose into thundering-herd, ramp-to-knee, and steady-state-soak.
- **All scale magnitudes** — <1k, ~10k, 100k+ bundles per run; always stream results to disk.
- **Realism + control** — dump-informed synthetic generator by default; `--corpus=<dir>` to replay real bundles; `--shape=<profile>` to target known weak points (giant-file, many-small, unicode-heavy, broken-binary, near-cap).
- **Compare two runs** — `--compare-to=<run-dir>` produces a delta report and exits non-zero past a regression threshold.
- **Auto-cleanup where safe, never on remote** — local-http and in-process bring up ephemeral storage and tear it down; remote-http tags every row with the run UUID and leaves it (operator runs `--cleanup-run=<uuid>` explicitly).

## Non-goals

- Prometheus push / Pushgateway / Grafana provisioning. Raw JSONL output, downstream ad-hoc tooling.
- Cloud-deployed load generators. The harness is operator-driven, not autonomous.
- Parser fuzzing. AFL / `cargo-fuzz` is a different tool with different invariants; not in this crate.
- WASM viewer perf testing.
- Replaying the 1.5GB SQL dump *as content*. We mine it for shape distributions only — bytes never leave that script.

## Architecture

```
crates/parse-load-harness/
├── src/
│   ├── main.rs               CLI entry, arg parse, dispatch
│   ├── config.rs             Args + RunConfig types
│   ├── corpus/
│   │   ├── mod.rs            trait Corpus { fn next_bundle() -> Bundle }
│   │   ├── synthetic.rs      dump-informed generator
│   │   ├── replay.rs         --corpus=<dir> directory replay
│   │   ├── shape.json        checked-in distributions (mined from pilot dump)
│   │   └── profiles/         hand-tuned override JSONs (giant-file.json, etc.)
│   ├── target/
│   │   ├── mod.rs            trait Target { async fn send_bundle(b) -> BundleResult }
│   │   ├── in_process.rs     constructs AppState, calls parse_session directly
│   │   ├── local_http.rs     manages compose lifecycle + HTTP client
│   │   └── remote_http.rs    HTTP client only, against external URL
│   ├── auth/
│   │   ├── mod.rs            trait Auth { fn apply(&self, req) }
│   │   ├── header.rs         X-Device-Id
│   │   └── mtls.rs           in-memory test CA + per-device leaf certs
│   ├── driver.rs             load loop: concurrency / ramp / soak / total
│   ├── reporter/
│   │   ├── mod.rs            per-bundle JSONL + summary.json
│   │   ├── system.rs         periodic /metrics or in-process sampler
│   │   └── compare.rs        --compare-to delta -> comparison.md
│   └── lib.rs                re-exports for the mine-dump binary
├── tests/
│   ├── synthetic_generator.rs
│   ├── driver_shape.rs       asserts ramp/soak math
│   └── compare_report.rs
├── bin/
│   └── mine-dump.rs          one-shot: SQL dump -> shape.json
├── infra/
│   └── compose/load-test.yml local-http target compose file
└── README.md                 operator runbook
```

### Key seams

- `Corpus::next_bundle() -> Bundle { device_id_template, zip_bytes, metadata }` — the driver doesn't know if it's getting synthetic or replayed.
- `Target::send_bundle(b) -> BundleResult { duration, parse_state, error?, http_status? }` — same trait for in-process, local-http, remote-http.
- `Auth::apply(req)` — stamps a request (HTTP) or a `DeviceIdentity` (in-process). Keeps OIDC bearer / future auth modes mechanical to add.
- The reporter consumes from an mpsc the driver pushes into — same producer/consumer pattern as the parse worker we just merged. Disk I/O never blocks the driver.

### Lifecycle

1. Parse args → `RunConfig`.
2. Resolve `target` (may bring up compose, open HTTP client, or build AppState).
3. Resolve `corpus` (load `shape.json` + RNG, or scan `--corpus` directory).
4. Spawn `system_sampler` task → writes `system.jsonl` on a timer.
5. Spawn `reporter` task → writes `bundles.jsonl` as bundles complete.
6. Run `driver` until `--total-bundles` or `--soak-minutes` reached.
7. Drain reporter, stop sampler, write `summary.json`.
8. If `--compare-to`, produce `comparison.md` (and set exit code).
9. Target teardown (compose down for local-http; nothing for remote).

## Components

### CLI surface

```
parse-load-harness run \
  --target=in-process|local-http|remote-http \
  [--target-url=https://pilot.cmtrace.net]      remote-http only
  [--auth=header|mtls]                          default header
  [--corpus=<dir>]                              default = synthetic
  [--shape=<profile>|<path>]                    default = shape.json
  --concurrency=<N>                             peak in-flight
  [--ramp-seconds=<T>]                          default 0 (thundering herd)
  [--total-bundles=<N> | --soak-minutes=<M>]    one required
  [--seed=<u64>]                                default = clock-based, logged
  [--out=<dir>]                                 default = ./runs/<utc-ts>-<uuid>/
  [--compare-to=<run-dir>]                      produces comparison.md
  [--regression-threshold=<pct>]                default 10
  [--strict]                                    fail on first error
  [--max-rss-mb=<N>] [--max-error-rate=<0..1>]  system thresholds
  [--no-cleanup]                                local-http: keep compose up
  [--sample-seconds=<T>]                        default 5
  [--in-process-pg=<url>]                       in-process only
  [--image=<docker-tag>]                        local-http only

parse-load-harness mine-dump <sql-file> [--out=shape.json]

parse-load-harness clean <target> --cleanup-run=<uuid>
```

### Corpus subsystem

`shape.json` schema (mined once from the 1.5GB pilot SQL dump via the `mine-dump` binary):

```json
{
  "files_per_bundle":     { "p50": 24, "p90": 38, "p99": 62, "max_observed": 87 },
  "bytes_per_file":       { "p50": 4200, "p90": 280000, "p99": 4_800_000 },
  "parser_kind_weights":  { "Ccm": 0.42, "IisW3c": 0.07, "Plain": 0.31 },
  "fallback_rate_per_kb": { "Ccm": 0.00005, "Plain": 0.0008 },
  "encoding_weights":     { "Utf8": 0.86, "Utf16Le": 0.13, "Other": 0.01 }
}
```

The synthetic generator samples per bundle from these distributions and produces zip bytes that match production shape — file count, parser-kind mix, fallback frequency. Lines are templated per kind (real CCM `<![LOG[...]LOG]!>` framing; real IIS W3C header + space-separated fields; etc.) so the parser does meaningful work.

`profiles/*.json` are hand-tuned overrides: `many-small` (200 files × 1 KiB), `giant-file` (1 file × 49 MiB), `unicode-heavy` (UTF-16LE bias), `broken-binary` (entries=0, errors>0 to drive `partial`), `near-cap` (just under `MAX_EVIDENCE_ZIP_BYTES`). Each profile overrides individual fields in `shape.json`.

`--corpus=<dir>` walks the directory, treats every `*.zip` ≤ `MAX_EVIDENCE_ZIP_BYTES` as a bundle, round-robins through them. Real bundles take precedence whenever provided.

The synthetic generator is deterministic given a seed. The seed is logged at run start and recorded in `summary.json` so any failing run is reproducible.

### Target adapters

| Target | Brings up | Tears down | Auth |
|---|---|---|---|
| `in-process` | tempdir blob store + ephemeral PG (or `--in-process-pg=<url>`); `AppState::new` with parse semaphore wired in | drops PG schema, removes tempdir | bypasses HTTP — stamps `DeviceIdentity` directly when calling `parse_session` |
| `local-http` | `docker compose up` of `infra/compose/load-test.yml` (api-server image + ephemeral PG + local FS blob); waits for `/healthz`; reads bound port | `docker compose down -v` | header default; `--auth=mtls` mints test CA, mounts on api-server, presents per-virtual-device leaf cert |
| `remote-http` | nothing (target must already be up) | nothing | header only; `--auth=mtls` rejected |

`infra/compose/load-test.yml` is new and local-only. The api-server service uses the locally-built image (or `--image=<tag>`).

### Load driver

```rust
async fn run_driver(target, corpus, cfg, reporter_tx) {
    let permit = Arc::new(Semaphore::new(0));         // start at 0, ramp adds permits
    let total_emitted = AtomicU64::new(0);

    spawn(ramp_task(permit.clone(), cfg));            // adds permits over time

    let mut tasks = JoinSet::new();
    let deadline = cfg.soak_minutes.map(|m| Instant::now() + m * 60);

    loop {
        if reached_total_or_deadline() { break }
        let owned = permit.clone().acquire_owned().await?;
        let bundle = corpus.next_bundle()?;
        tasks.spawn(async move {
            let _p = owned;
            let result = target.send_bundle(bundle).await;
            reporter_tx.send(result).await.ok();
        });
    }
    while tasks.join_next().await.is_some() {}        // drain
}
```

Mirrors the producer/permit pattern just merged into api-server's parse worker. Total cap = `--concurrency`. Ramp adds permits gradually so cold-start contention shows up in the data instead of being smoothed over. Soak mode = same loop, `total_bundles=∞`, exits on deadline.

### Reporter + system sampler

**bundles.jsonl** — one row per completed bundle:
```json
{"t":"2026-04-29T19:14:32.412Z","run":"<uuid>","seq":4187,"device":"LOAD-TEST-0042",
 "n_files":24,"bundle_bytes":4812000,"finalize_ms":120,"parse_ms":3450,
 "parse_state":"ok-with-fallbacks","files_with_fallbacks":3,
 "http_status":201,"error":null}
```

**system.jsonl** — one row every `--sample-seconds` (default 5):
```json
{"t":"2026-04-29T19:14:35.000Z","in_flight":47,"semaphore_avail":1,
 "rss_mb":2840,"db_pool_idle":3,"db_pool_active":12,
 "cmtrace_parse_worker_inflight":4,"cmtrace_parse_worker_runs_total":4187}
```

Source: `/metrics` scrape for HTTP targets; in-process exposes the same counters via a dev-only handle on `AppState`. (Implementation note: `AppState` currently exposes `metrics: PrometheusHandle` for the `/metrics` route; the in-process sampler reuses that handle directly. RSS for in-process is read via `procfs` / `mach_task_self` rather than the metrics recorder.)

**summary.json** at end of run: counts by parse_state, p50/p95/p99 of finalize_ms and parse_ms, throughput (bundles/min), peak in-flight, peak RSS, error count, seed, total wall time, target/auth/concurrency configuration.

**comparison.md** when `--compare-to=<dir>`: tabular delta of every summary metric, percent change, regression flags past `--regression-threshold` (default 10%). Sets exit code 1 if any flagged.

### Auth path

- `--auth=header` (default): `X-Device-Id: LOAD-TEST-<run>-<seq % n_devices>`. `n_devices` defaults to `concurrency` so the harness simulates `concurrency` distinct devices, not a single device flooding.
- `--auth=mtls` (local-http only): startup mints a single CA in-memory using `rcgen`, generates per-virtual-device leaf certs, writes the CA PEM to a tempfile, mounts it on the api-server compose service via `CMTRACE_MTLS_TRUST_ROOTS=...`. No openssl dep. Exercises the `DeviceIdentity` extractor + the CRL cache (with empty CRL).

## Data flow

```
config -> target adapter ──┐
                           │       ┌── reporter ── bundles.jsonl
config -> corpus ──┐       │       │             ── summary.json
                   ├──> driver ────┤
seed -> RNG ───────┘       │       └── system_sampler ── system.jsonl
                           │
                           └── (HTTP only) /metrics scrape
```

## Error handling

| Failure mode | Harness response | Exit code |
|---|---|---|
| Target unreachable at startup | log, abort | 2 |
| Corpus invalid (empty dir, malformed shape.json) | log, abort | 2 |
| One bundle errors (HTTP 5xx, timeout, parse_state=failed) | record in bundles.jsonl, continue | 0 unless `--strict` or rate > `--max-error-rate` (then 1) |
| All bundles errored | record, write summary, exit 1 | 1 |
| Reporter task panics | log, abort, attempt to flush bundles.jsonl | 2 |
| `--max-rss-mb` exceeded mid-run | log, finish in-flight, abort gracefully | 1 |
| `--compare-to` regression past threshold | run completes normally; comparison.md flags; exit 1 | 1 |

Defaults err loose for benchmark mode (errors reported but exit 0 unless rate > 1%); `--strict` fails on the first error.

## Testing

The harness ships with three test files:

1. **`tests/synthetic_generator.rs`** — given a fixed seed, the generator produces byte-identical zips run-to-run; samples from `shape.json` actually match its target distributions over N draws (chi-square or KS test loose threshold); pathological profiles (`giant-file`, `near-cap`) produce bundles inside the 50 MiB cap.

2. **`tests/driver_shape.rs`** — pure-logic tests of the driver's load shape: thundering-herd (ramp_seconds=0) emits all `concurrency` permits at t=0; ramp distributes permit additions linearly; soak mode honors deadline; `--total-bundles` cap is exact (no over-shoot).

3. **`tests/compare_report.rs`** — `compare(prev, curr)` flags regressions correctly past the threshold; identical runs report 0% delta on every metric; missing keys handled gracefully (don't panic on schema drift).

The harness itself does not get a CI gate as part of this design — that's a follow-up. The unit tests run on every push as part of the workspace `cargo test`.

## Hooks for future codebase improvement

These are not in scope to *implement* in this design, but the harness is purpose-built to surface them cleanly:

1. **Parser hot loops** — flame graph the harness with `samply` against `--target=in-process --concurrency=1` to find which parser kinds dominate.
2. **DB query plans** — system.jsonl will surface slow `insert_entries_batch` commits; profile with `auto_explain` on local PG.
3. **Blob backend choice** — compare local-http with `BLOB_BACKEND=fs` vs `BLOB_BACKEND=azurite` to decompose latency.
4. **mTLS overhead** — `--auth=mtls` vs header on the same load surfaces cert validation cost under fan-out.
5. **Semaphore tuning** — sweep `CMTRACE_PARSE_CONCURRENCY` across runs; use `--compare-to` to find the knee.
6. **CI regression gate** (eventual) — `cargo run -p parse-load-harness -- run --target=in-process --total-bundles=200 --concurrency=8 --compare-to=baseline` becomes a soft regression check on PRs.

## Open questions deferred

- **Baseline ownership:** where the canonical `--compare-to` baseline lives (a dedicated branch's `runs/` dir? S3? Git LFS?) — the spec doesn't decide; first time we want a CI gate, decide then.
- **Test CA reuse vs per-run mint:** today's design mints per-run. If startup latency becomes a problem under thousands of `--auth=mtls` runs, we could checkpoint a CA and re-mint only leaves. Defer until measured.
- **Real bundles → checked-in golden corpus:** once the user collects bundles from the test fleet, we'll evaluate whether to commit a small golden set to the repo (probably with Git LFS) or keep `--corpus=<dir>` strictly local. Defer to that point.
