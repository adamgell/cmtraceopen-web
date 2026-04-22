# Wave 4 — Bundle Retention

> **Audience:** operators of cmtraceopen-web deployments and the next
> contributor extending the storage layer. This doc closes the
> ship-blocker called out in
> [Day-2 Operations Runbook §7.2](04-day2-operations.md#72-blob-store):
> the `CMTRACE_BUNDLE_TTL_DAYS` env var was documented as the intended
> knob but had no implementation. Wave 4 ships both halves of a
> two-tier retention design.

## Problem

Today blob storage grows unbounded. The walking skeleton on BigMac26
appends every finalized bundle into `/data/blobs/<session_id>` with no
cleanup path. At the documented per-fleet sizing of `100 dev × 4
bundles × 5 MB × 30 d ≈ 60 GB / month` ([§4.1 capacity
planning](04-day2-operations.md#41-per-tier-ceilings)), a 90-day
retention horizon is the difference between "the runner host's
500 GB SSD lasts a year" and "operator notices when the disk is full".

The runbook's interim workaround was an out-of-band `find -mtime
+90 -delete`, with the explicit caveat that purging blobs while the
metadata store still references them surfaces 404s in the viewer. We
need an in-process sweeper that purges the metadata row alongside the
blob.

## Two-tier design

### Tier 1 — Server-side sweeper (works for every blob backend)

A new tokio task spawned in `main.rs` alongside the parse worker
pool. Wakes up every `CMTRACE_RETENTION_SCAN_INTERVAL_SECS` (default
21 600 s = 6 hours) and runs one scan:

1. Ask `MetadataStore::sessions_older_than(ttl_days, batch_size)` for
   up to `CMTRACE_RETENTION_BATCH_SIZE` (default 100) sessions whose
   `ingested_utc < now - ttl_days`. Oldest-first ordering so the
   sweeper makes monotonic progress through a backlog.
2. For each candidate:
   1. `BlobStore::head_blob(uri)` to capture the byte size for the
      "bytes freed" metric. A 404 is treated as 0 bytes (the blob is
      already gone — likely a previous sweep crashed mid-cycle, see
      [Idempotency](#idempotency)).
   2. `BlobStore::delete_blob(uri)` to hard-delete the bundle.
      Implementations MUST treat "blob not found" as success.
   3. `MetadataStore::delete_session(session_id)` to clear the
      session row plus its `files` and `entries` fan-out, all under a
      single SQLite transaction.
3. Emit per-session `info!` lines (one per success) and `warn!` lines
   (one per failure). Bump the four metrics below. Log a summary
   `info!` at the end of the scan with `candidates`, `deleted`, and
   `errors` counts.

The sweeper is **per-session transactional**, not per-batch. A crash
between blob delete and metadata delete leaves the row pointing at a
missing blob — see [Idempotency](#idempotency) for why this is
acceptable.

#### Configuration surface

| Env var | Default | Purpose |
| --- | --- | --- |
| `CMTRACE_BUNDLE_TTL_DAYS` | `90` | Sessions older than this in days are eligible for purge. `0` disables the sweeper entirely (loop runs but no-ops every tick). |
| `CMTRACE_RETENTION_SCAN_INTERVAL_SECS` | `21600` | How often the sweeper wakes up to look for work. |
| `CMTRACE_RETENTION_BATCH_SIZE` | `100` | Cap on sessions purged per scan. Prevents the first sweep on a backlog-heavy deployment from holding the sessions table for minutes. |

#### Metrics

All four are Prometheus counters exposed via the existing
`/metrics` endpoint, with describes registered in
`main.rs::describe_metrics`:

- `cmtrace_retention_sweeps_total` — sweep passes since process start
  (one per scan tick).
- `cmtrace_retention_sessions_deleted_total` — sessions hard-deleted
  (blob + metadata).
- `cmtrace_retention_bytes_freed_total` — approximate freed bytes,
  summed from `head_blob` before delete.
- `cmtrace_retention_errors_total{stage="scan|blob|metadata"}` —
  failure counter, labeled by where in the per-session pipeline the
  error landed.

Suggested alert (Grafana / Prometheus):

```promql
# Retention has not made progress in 24 h and the deployment
# has a non-trivial backlog. Sev 3 — disk usage degrades, ingest
# is unaffected.
increase(cmtrace_retention_sessions_deleted_total[24h]) == 0
  and increase(cmtrace_retention_sweeps_total[24h]) > 0
```

### Tier 2 — Azure Blob lifecycle policy (cloud-only, optional)

For deployments running with `CMTRACE_BLOB_BACKEND=Azure`, the storage
account itself can enforce blob aging at zero compute cost. The
sweeper still has to run for metadata cleanup (the `sessions` /
`files` / `entries` rows), but offloading the blob delete to Azure's
lifecycle engine has two benefits:

- **Cool / Cold tiering** — blobs that haven't been read in 30 days
  drop to Cool tier (cheaper storage, slightly higher read cost). The
  parse worker only touches a blob once at ingest, so post-parse the
  bundle is read at most occasionally for ad-hoc forensic re-parse;
  Cool is the right shape.
- **Hard-delete after 90 days** — Azure deletes the blob even if the
  api-server is down. This is the disaster-recovery story for
  "operator forgot the sweeper was disabled".

Configure the lifecycle policy via Terraform alongside the storage
account. A drop-in for `infra/azure/modules/storage/` (when that
module lands; currently the storage account is provisioned by hand
per [Day-2 §1](04-day2-operations.md#1-topology-recap)):

```hcl
resource "azurerm_storage_management_policy" "cmtraceopen_lifecycle" {
  storage_account_id = azurerm_storage_account.cmtraceopen.id

  rule {
    name    = "cmtraceopen-bundle-aging"
    enabled = true
    filters {
      prefix_match = ["${var.container_name}/blobs/"]
      blob_types   = ["blockBlob"]
    }
    actions {
      base_blob {
        # Move to Cool tier 30 days after the blob was last modified.
        tier_to_cool_after_days_since_modification_greater_than = 30
        # Hard-delete 90 days after last modification — matches the
        # default CMTRACE_BUNDLE_TTL_DAYS so server + cloud agree.
        delete_after_days_since_modification_greater_than = 90
      }
    }
  }
}
```

**Operators running on Azure SHOULD use both tiers.** The sweeper
keeps the metadata store small (queries fast, backups cheap); the
lifecycle policy keeps blob storage cost predictable even if the
sweeper is disabled or the api-server is down for an extended
window. They don't conflict — `delete_blob` on an already-gone blob
is a no-op (Azure returns 404, mapped to `Ok(())` in
`ObjectStoreBlobStore::delete_blob`).

For local-FS deployments tier 2 doesn't exist — the sweeper is the
only cleanup path.

## Decisions

### Default TTL = 90 days

Aligns with the Day-2 runbook's [§7.2 blob store retention
guidance](04-day2-operations.md#72-blob-store) which already
documented "Default (today): 90 days. Older blobs are eligible for
purge." Postgres backup retention is 30 days per [§7.1](
04-day2-operations.md#71-postgres-metadata), so blobs survive their
metadata's last backup window by 60 days — enough for "restore from
backup, re-parse old bundles" workflows.

Operators with regulatory retention requirements (HIPAA, SOX, GDPR
right-to-erasure) should override per-deployment. Set to `0` to
disable sweeping entirely.

### Hard delete, not soft delete

`delete_blob` removes the bundle bytes; `delete_session` removes the
metadata. There is no `deleted_utc` tombstone column. Once a bundle
is past TTL, it is unrecoverable — the operator cannot re-parse, and
the viewer cannot reconstruct any view of the session.

This is a deliberate simplification:

- The MVP has no "I want it back" workflow that would consume a
  tombstone.
- Adding a `deleted_utc` column later is a single migration; nothing
  prevents the upgrade.
- Soft-delete on a 60-GB-per-month firehose just defers the disk
  problem.

If a soft-delete pattern is needed later, add `deleted_utc TEXT NULL`
to `sessions`, change the sweeper to issue an UPDATE instead of a
DELETE, and add a second background task that hard-deletes rows where
`deleted_utc < now - tombstone_ttl`.

### Idempotency

The sweeper is crash-safe at the per-session boundary, not the
per-batch boundary. There are three failure modes worth calling out:

1. **`head_blob` returns 404** — the blob was already deleted, likely
   by a previous sweep that crashed before clearing the metadata row.
   Treat as 0 bytes freed and proceed with `delete_blob` (which will
   also no-op) and `delete_session` (which finally removes the
   lingering row). Convergent.
2. **`delete_blob` fails** — log + count under
   `cmtrace_retention_errors_total{stage="blob"}`, leave the metadata
   row in place, retry on the next scan. Convergent.
3. **`delete_blob` succeeds, `delete_session` fails** — the blob is
   gone but the row remains. The viewer will surface 404s for that
   session until the next scan re-runs the cycle (`head_blob` returns
   404 → continue → `delete_session` succeeds this time). Convergent
   within one scan interval (default 6 h).

This is weaker than a two-phase commit, but the only scenario where
two-phase commit would help is "the operator wants to know in real
time about partial-deletion sessions." That signal is already
available via the metric.

### Why a separate task instead of multiplexing into the parse-worker pool

Parse work is CPU-bound, fire-and-forget per session, and bursts on
the inbound ingest path. Retention work is I/O-bound (one DB query,
one network delete per session), wall-clock-triggered, and runs at a
constant low rate independent of ingest. Putting them on the same
queue would let a retention pause (e.g. transient Azure throttle) back
up parse work and inflate ingest finalize latency — an undesirable
coupling. Two tasks, two metric namespaces, one shared `Arc<dyn
BlobStore>` and `Arc<dyn MetadataStore>`.

## Migration path

### Existing deployments

Existing deployments without `CMTRACE_BUNDLE_TTL_DAYS` set get the
default 90 days **immediately on upgrade**. Operators who want to
disable the sweeper must set `CMTRACE_BUNDLE_TTL_DAYS=0` explicitly.

For the BigMac26 walking skeleton specifically: the runner has been
ingesting bundles since the Wave 3 mTLS cutover. Most data is well
under 90 days old, so the first sweep should be a no-op or near-no-op.
Operators on long-running deployments should:

1. Snapshot the blob store before the upgrade:
   ```bash
   sudo rsync -avz /var/lib/cmtraceopen/data/blobs/ \
     /backup/cmtraceopen/pre-retention-rollout/
   ```
2. Watch the first sweep's summary log line:
   ```bash
   docker compose logs api-server --since=10m | grep "retention sweep"
   ```
3. Confirm `cmtrace_retention_errors_total` stays at 0 across the
   first 24 h.

### First-sweep thundering herd

`CMTRACE_RETENTION_BATCH_SIZE` caps any single scan at 100 sessions
by default. A backlog of 10 000 expired sessions takes ~100 scans to
work through, which at the default 6-hour interval is ~25 days.
Operators with massive backlogs and disk pressure should temporarily
shrink the interval (`CMTRACE_RETENTION_SCAN_INTERVAL_SECS=300`,
5 minutes) and bump the batch (`CMTRACE_RETENTION_BATCH_SIZE=1000`)
until the backlog drains, then revert.

## Test plan

Three unit tests live in
`crates/api-server/src/pipeline/retention.rs#tests`, all driving the
real `SqliteMetadataStore` against `:memory:` plus a
hand-rolled in-process `BlobStore` mock:

1. **`sweeper_deletes_old_sessions`** — seeds 5 sessions (3 dated
   60 d ago, 2 dated 1 d ago), runs `sweep_once(ttl=30, batch=100)`,
   asserts exactly 3 metadata rows and 3 blobs are gone, the other 2
   intact.
2. **`sweeper_skips_when_ttl_zero_or_unset`** — seeds 2 ancient
   sessions, mirrors the `run_retention_loop` gate (skip
   `sweep_once` when `ttl == 0`), asserts nothing is deleted.
3. **`sweeper_idempotent_after_partial_failure`** — seeds 1 old
   session, arms the mock blob store to fail the first
   `delete_blob` call only, asserts:
   - first sweep reports 0 deletions, leaves the metadata row,
     records 1 delete attempt.
   - second sweep reports 1 deletion, removes the row + blob,
     records 2 total delete attempts.

Run with: `cargo test -p api-server pipeline::retention`.

## Future work

- **Postgres backend** — when `MetadataStore` gets a Postgres impl,
  rewrite `sessions_older_than` to use `now() - interval '$1 days'`
  rather than SQLite's `datetime('now', '-N days')`. The trait shape
  doesn't change.
- **Per-device retention** — operators may want different TTLs for
  different device classes (lab machines: 30 d, prod: 365 d). Add a
  `retention_days` column on `devices` and have
  `sessions_older_than` join against it. Not in scope for the MVP.
- **Soft delete with tombstone GC** — see
  [Hard delete, not soft delete](#hard-delete-not-soft-delete).
- **Lifecycle-policy Terraform module** — the snippet above lives in
  this doc only. A proper `infra/azure/modules/storage/` module that
  takes `bundle_ttl_days` as an input and renders the lifecycle JSON
  is the next step once the storage account itself is
  Terraform-managed.
