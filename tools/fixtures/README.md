# test-bundle fixture

A tiny, reproducible evidence bundle used by `tools/ship-bundle.sh` to
smoke-test the api-server's ingest pipeline.

## Contents

```
manifest.json                     static metadata, see schema below
evidence/logs/test.log            three CMTrace <![LOG[...]LOG]!> lines
```

Both files have a fixed mtime pinned to `SOURCE_DATE_EPOCH` (default:
`1767225600` = `2026-01-01T00:00:00Z`), and the archive is built with
`zip -X -D` to strip extra fields and directory entries — so the output
is byte-reproducible across Linux and macOS runners.

## Generating

```bash
bash tools/fixtures/build.sh
```

Prints the output path, byte size, and sha256. The generated
`tools/fixtures/test-bundle.zip` is gitignored; each developer / CI run
rebuilds it from `build.sh`. That keeps the repo lean and forces the
script itself to be the source of truth.

Expected output (as of this README; re-run and paste if it drifts):

```
built: .../tools/fixtures/test-bundle.zip
size : ~540 bytes
sha256: <computed at build time>
```

The exact size and sha are whatever `build.sh` produces on a clean
Linux/macOS box. CI should run `build.sh` fresh and use the output,
rather than trusting a checked-in value.

## manifest.json schema

Mirrors (a subset of) what the Windows agent will emit once it lands.
Keep this schema tight; the server does not parse it yet (parse_state
is always `"pending"` in MVP), but the agent contract should treat
these fields as required.

| field          | type   | notes                                           |
|----------------|--------|-------------------------------------------------|
| `schemaVersion`| int    | bump on breaking manifest changes               |
| `bundleKind`   | string | one of `evidence-zip`, `ndjson-entries`, `raw-file` — matches `contentKind` wire constant |
| `collectedUtc` | string | RFC3339 timestamp, when the agent snapshotted   |
| `agent.name`   | string | identifier for the collector                    |
| `agent.version`| string | semver                                          |
| `device.hostname` | string | short hostname                               |
| `device.os`    | string | human-readable OS name                          |
| `device.osVersion` | string | build string                                |
| `artifacts[]`  | array  | entries describing each file in the bundle      |
| `artifacts[].path` | string | path relative to the archive root           |
| `artifacts[].kind` | string | e.g. `cmtrace-log`, `evtx`, `registry-hive` |
| `artifacts[].description` | string | short human note                      |

## Extending

To add an artifact: edit `build.sh`, create the file under a new staging
path, and append an entry to `artifacts[]` in the inline `manifest.json`
heredoc. Keep the total archive under 1 MiB so this fixture stays cheap
to ship around.
