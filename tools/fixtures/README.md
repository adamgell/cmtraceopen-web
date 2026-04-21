# test-bundle fixture

A tiny, reproducible evidence bundle used by `tools/ship-bundle.sh` to
smoke-test the api-server's ingest pipeline, and by the wasm parser
canary (`scripts/wasm-smoke.mjs`) to catch parser regressions in CI.

The bundle is deliberately broad rather than deep: one valid record per
parser shape, plus a fake `dsregcmd /status` and an event-log placeholder,
so a regression in any parser surfaces here before reaching production
evidence.

## Contents

```
manifest.json                              static metadata, see schema below
evidence/logs/ccm.log                      three CMTrace <![LOG[...]LOG]!> records
evidence/logs/cbs.log                      three CBS servicing records
evidence/logs/panther.log                  three Panther setupact-style records
evidence/logs/plain.log                    three free-form lines (plain fallback)
evidence/command-output/dsregcmd-status.txt  fake `dsregcmd /status` output
evidence/event-logs/system.txt             placeholder explaining why no .evtx
```

All files have mtimes pinned to `SOURCE_DATE_EPOCH` (default:
`1767225600` = `2026-01-01T00:00:00Z`), and the archive is built with
`zip -X -D` to strip extra fields and directory entries — so the output
is byte-reproducible across Linux, macOS, and Windows (Git Bash) runners.

### Per-file expected shapes

| Path | Parser | Records | Notable fields |
|------|--------|---------|----------------|
| `evidence/logs/ccm.log` | CCM (`ParserKind::Ccm`) | 3 | `component` ∈ {CcmExec, PolicyAgent}; severities Info/Info/Error; threads 100,101,100 |
| `evidence/logs/cbs.log` | CBS | 3 | sources {CBS, CSI, CBS}; severities Info/Warning/Error |
| `evidence/logs/panther.log` | Panther setup | 3 | sources {MIG, SP, SP}; severities Info/Warning/Error; record 1 has hex code `[0x080489]` |
| `evidence/logs/plain.log` | Plain fallback | 3 | unstructured; detector drops to plain-text |
| `evidence/command-output/dsregcmd-status.txt` | dsregcmd | — | `AzureAdJoined=YES`, `DomainJoined=NO`, `WorkplaceJoined=NO`, all-zero GUIDs for `TenantId` / `DeviceId`, `AzureAdPrt=YES` in both User and SSO sections, `KeySignTest=PASSED`, `User Context=SYSTEM` |
| `evidence/event-logs/system.txt` | — (placeholder) | — | See "Why no .evtx?" below |

Counts are stable and deterministic: the wasm canary could be extended
to assert `entries.length` per format (see `scripts/wasm-smoke.mjs` —
today only `ccm.log` is asserted; extending to the others is tracked as a
follow-up).

### Why no .evtx?

Event-log coverage ships as a `.txt` placeholder, not a real `.evtx`,
because:

1. Even a single-record System.evtx is ~32 KiB, which would blow this
   fixture's <5 KiB budget by an order of magnitude.
2. EVTX output is not byte-reproducible: chunk headers embed creation
   timestamps and channel GUIDs that vary per `wevtutil` /
   PowerShell invocation, so the zip's sha256 would drift per build.

The production contract (documented in the placeholder file itself) is:
the Windows collector writes raw `.evtx` under `evidence/event-logs/` AND
emits flattened JSON siblings under
`analysis-input/event-logs/<channel>.json` that the wasm analyzer
consumes. Fixtures exercising the event-log analyzer should ship a JSON
sibling alongside this placeholder.

## Generating

```bash
bash tools/fixtures/build.sh
```

Prints the output path, byte size, and sha256. The generated
`tools/fixtures/test-bundle.zip` is gitignored; each developer / CI run
rebuilds it from `build.sh`. That keeps the repo lean and forces the
script itself to be the source of truth.

The script enforces a 5 KiB size ceiling; if `build.sh` trips that guard,
either trim content or bump the budget intentionally in the script.

### Reproducibility check

Two back-to-back builds must produce an identical sha256:

```bash
bash tools/fixtures/build.sh | tee /tmp/a
bash tools/fixtures/build.sh | tee /tmp/b
diff <(grep '^sha256:' /tmp/a) <(grep '^sha256:' /tmp/b)   # must be empty
```

CI runs this as a gate.

## manifest.json schema

Mirrors (a subset of) what the Windows agent will emit once it lands.
Keep this schema tight; the server does not parse it yet (parse_state
is always `"pending"` in MVP), but the agent contract should treat
these fields as required.

| field               | type   | notes                                                                                  |
|---------------------|--------|----------------------------------------------------------------------------------------|
| `schemaVersion`     | int    | bump on breaking manifest changes                                                      |
| `bundleKind`        | string | one of `evidence-zip`, `ndjson-entries`, `raw-file` — matches `contentKind` wire const |
| `collectedUtc`      | string | RFC3339 timestamp, when the agent snapshotted                                          |
| `collectorVersion`  | string | semver-ish tag of the collector binary (fixture uses `0.0.0-fixture`)                  |
| `agent.name`        | string | identifier for the collector                                                           |
| `agent.version`     | string | semver                                                                                 |
| `device.hostname`   | string | short hostname                                                                         |
| `device.os`         | string | human-readable OS name                                                                 |
| `device.osVersion`  | string | build string                                                                           |
| `artifacts[]`       | array  | entries describing each file in the bundle                                             |
| `artifacts[].path`  | string | path relative to the archive root                                                      |
| `artifacts[].kind`  | string | e.g. `cmtrace-log`, `cbs-log`, `panther-setup-log`, `plain-text-log`, `dsregcmd-status`, `event-log-placeholder` |
| `artifacts[].description` | string | short human note                                                                 |

## Extending

To add an artifact: edit `build.sh`, create the file under a new staging
path, and append an entry to `artifacts[]` in the inline `manifest.json`
heredoc. Keep the total archive under 5 KiB so this fixture stays cheap
to ship around — the size guard in `build.sh` will fail the build if you
exceed the budget.

Constraints to respect when adding content:

* **No binary fixtures over a few hundred bytes.** Prefer plain-text
  analogs with a placeholder explaining the real production format.
* **Deterministic only.** No `$(date)`, no UUIDs, no random. Every value
  must be a frozen literal or derived from `SOURCE_DATE_EPOCH`.
* **At most one record per parser shape** unless you're deliberately
  testing multi-line / continuation behavior. Breadth over depth.
