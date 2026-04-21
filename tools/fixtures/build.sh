#!/usr/bin/env bash
# Build the reference test-bundle.zip used by ship-bundle.sh and CI.
#
# Goals:
#   * Reproducible: byte-identical output across machines so the committed
#     fixture's sha256 can be asserted in CI.
#   * Tiny: <1 MiB, a handful of lines of fake CCM log + a manifest.
#   * No exotic deps: bash, zip, sha256sum, date. jq is NOT required here.
#
# Reproducibility tricks:
#   * All mtimes are pinned to SOURCE_DATE_EPOCH (default: a fixed constant).
#   * `zip -X` strips extra fields (e.g. uid/gid, local extended timestamps).
#   * File order inside the archive is sorted explicitly.
#   * TZ=UTC + LC_ALL=C to keep any tool output deterministic.

set -euo pipefail

# 2026-01-01T00:00:00Z -- arbitrary but fixed.
: "${SOURCE_DATE_EPOCH:=1767225600}"
export TZ=UTC
export LC_ALL=C

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${HERE}/test-bundle.zip"
STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT

mkdir -p "${STAGE}/evidence/logs"

# --- manifest.json ----------------------------------------------------------
# Static fields so hash stays constant. Schema mirrors what the agent will
# emit; extend here as the real manifest grows.
cat > "${STAGE}/manifest.json" <<'JSON'
{
  "schemaVersion": 1,
  "bundleKind": "evidence-zip",
  "collectedUtc": "2026-01-01T00:00:00Z",
  "agent": {
    "name": "cmtraceopen-test-fixture",
    "version": "0.0.0"
  },
  "device": {
    "hostname": "TEST-FIXTURE",
    "os": "Windows 11 Pro",
    "osVersion": "10.0.26200"
  },
  "artifacts": [
    {
      "path": "evidence/logs/test.log",
      "kind": "cmtrace-log",
      "description": "Synthetic CCM log with three LogEntry lines."
    }
  ]
}
JSON

# --- evidence/logs/test.log -------------------------------------------------
# A few lines in the CMTrace <![LOG[...]LOG]!> format. Deliberately short so
# downstream parsers can assert on exact content if they want.
cat > "${STAGE}/evidence/logs/test.log" <<'LOG'
<![LOG[CMTraceOpen test fixture - line 1]LOG]!><time="00:00:00.000+000" date="01-01-2026" component="test" context="" type="1" thread="1" file="test.cpp:1">
<![LOG[CMTraceOpen test fixture - line 2 (warning)]LOG]!><time="00:00:01.000+000" date="01-01-2026" component="test" context="" type="2" thread="1" file="test.cpp:2">
<![LOG[CMTraceOpen test fixture - line 3 (error)]LOG]!><time="00:00:02.000+000" date="01-01-2026" component="test" context="" type="3" thread="1" file="test.cpp:3">
LOG

# Pin mtimes so zip headers are deterministic.
find "${STAGE}" -exec touch -d "@${SOURCE_DATE_EPOCH}" {} +

# Build zip deterministically:
#   -X  strip extra fields (uid/gid, extended timestamps)
#   -D  no directory entries in archive
#   -q  quiet
# File list is sorted so archive ordering is stable.
rm -f "${OUT}"
(
  cd "${STAGE}"
  find . -type f | LC_ALL=C sort | sed 's|^\./||' | \
    zip -X -D -q "${OUT}" -@
)

SIZE=$(wc -c < "${OUT}" | tr -d '[:space:]')
SHA=$(sha256sum "${OUT}" | awk '{print $1}')

echo "built: ${OUT}"
echo "size : ${SIZE} bytes"
echo "sha256: ${SHA}"
