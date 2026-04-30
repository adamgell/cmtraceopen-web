# parse-load-harness

Drives the api-server's parse pipeline against three targets with three load
shapes and streams structured per-bundle and system-metric output to disk.
See [the design doc](../../docs/superpowers/specs/2026-04-29-parse-load-harness-design.md).

## Quick recipes

### Repeatable in-process benchmark

You'll need a scratch Postgres reachable at the URL you pass.

```sh
cargo run --release -p parse-load-harness -- run \
  --target in-process \
  --in-process-pg postgres://cmtrace:cmtrace@localhost:5432/cmtrace_loadtest \
  --concurrency 8 \
  --total-bundles 200 \
  --seed 42 \
  --out runs/baseline-in-process
```

### Compare two runs

```sh
cargo run --release -p parse-load-harness -- run \
  --target in-process \
  --in-process-pg ... --concurrency 8 --total-bundles 200 --seed 42 \
  --compare-to runs/baseline-in-process \
  --out runs/optimization-test \
  --regression-threshold 5
```

Exit code 1 iff a regression beyond 5% is detected.

### Local stack (full HTTP path)

```sh
cargo build -p api-server --release
docker build -t cmtraceopen-api:latest -f crates/api-server/Dockerfile .

cargo run --release -p parse-load-harness -- run \
  --target local-http \
  --concurrency 32 \
  --soak-minutes 10 \
  --image cmtraceopen-api:latest
```

### Soak against a deployed environment (read-only — leaks tagged rows)

```sh
cargo run --release -p parse-load-harness -- run \
  --target remote-http \
  --target-url https://pilot.cmtrace.net \
  --concurrency 16 \
  --soak-minutes 30
```

Run-tagged rows persist; clean them up later with `clean --cleanup-run=<uuid>` (NOT YET IMPLEMENTED — see follow-ups).

### Stress profiles

```sh
... --shape giant-file
... --shape many-small
... --shape unicode-heavy
... --shape broken-binary
... --shape near-cap
... --shape /path/to/custom-shape.json
```

### Real-bundle replay

Drop `*.zip` files into a directory:

```sh
... --corpus ./real-bundles
```

`--shape` is ignored when `--corpus` is given.

## Output

Every run writes three files to `--out`:

- `bundles.jsonl` — one row per bundle (seq, parse_state, finalize_ms, parse_ms, error)
- `system.jsonl` — periodic samples (every `--sample-seconds`)
- `summary.json` — aggregate stats (counts by state, p50/p95/p99, throughput, seed, run UUID)

When `--compare-to` is given, also `comparison.md`.

## Mining a shape from a Postgres dump

```sh
cargo run --release -p parse-load-harness --bin mine-dump -- \
  ./pilot-2026-04-28.sql \
  --pg-url postgres://cmtrace:cmtrace@localhost:5432/scratch \
  --out crates/parse-load-harness/src/corpus/shape.json
```

The resulting `shape.json` is checked in. Re-run when you have a new dump.

## Known follow-ups

The `clean` subcommand is stubbed but not yet implemented. The seed `shape.json` checked in here is hand-tuned; running `mine-dump` against the real pilot SQL dump will replace it with measured distributions.
