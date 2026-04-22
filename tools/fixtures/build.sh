#!/usr/bin/env bash
# Build the reference test-bundle.zip used by ship-bundle.sh and CI.
#
# Goals:
#   * Reproducible: byte-identical output across machines so the committed
#     fixture's sha256 can be asserted in CI.
#   * Tiny: <5 KiB — a manifest, a handful of log lines in various formats,
#     a fake dsregcmd /status output, and a placeholder for event logs.
#   * No exotic deps: bash, zip, sha256sum, date. jq is NOT required here.
#
# Reproducibility tricks:
#   * All mtimes are pinned to SOURCE_DATE_EPOCH (default: a fixed constant).
#   * `zip -X` strips extra fields (e.g. uid/gid, local extended timestamps).
#   * File order inside the archive is sorted explicitly.
#   * TZ=UTC + LC_ALL=C to keep any tool output deterministic.
#
# Why the varied content?
#   The web parser ships several format-specific implementations (CCM, CBS,
#   Panther setup logs, plain-text fallback) plus the dsregcmd parser and a
#   future Windows Event Log analyzer. A single synthetic CCM file only
#   exercises one happy path. This bundle is small but broad: at least one
#   valid record per parser shape, so wasm / analyzer regressions show up
#   here before they reach production evidence.
#
# Binary fixtures (*.evtx, *.cab, registry hives) are deliberately NOT
# included — even a minimal .evtx is ~32 KiB and can't be produced
# deterministically on Linux runners. For event-log coverage we ship a
# plain-text placeholder explaining the contract; production agents will
# attach pre-parsed `analysis-input/*.json` alongside the raw binary.

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
mkdir -p "${STAGE}/evidence/command-output"
mkdir -p "${STAGE}/evidence/event-logs"

# --- manifest.json ----------------------------------------------------------
# Static fields so hash stays constant. Schema mirrors what the agent will
# emit; extend here as the real manifest grows.
#
# `collectedUtc` and `collectorVersion` are frozen so the bundle stays
# byte-reproducible even as the real collector's version ticks forward.
cat > "${STAGE}/manifest.json" <<'JSON'
{
  "schemaVersion": 1,
  "bundleKind": "evidence-zip",
  "collectedUtc": "2026-01-01T00:00:00Z",
  "collectorVersion": "0.0.0-fixture",
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
      "path": "evidence/logs/ccm.log",
      "kind": "cmtrace-log",
      "description": "Synthetic CCM log with three LogEntry lines."
    },
    {
      "path": "evidence/logs/cbs.log",
      "kind": "cbs-log",
      "description": "Synthetic CBS servicing log with three records."
    },
    {
      "path": "evidence/logs/panther.log",
      "kind": "panther-setup-log",
      "description": "Synthetic Panther setupact.log with two records."
    },
    {
      "path": "evidence/logs/plain.log",
      "kind": "plain-text-log",
      "description": "Free-form text for the plain fallback parser."
    },
    {
      "path": "evidence/command-output/dsregcmd-status.txt",
      "kind": "dsregcmd-status",
      "description": "Synthetic `dsregcmd /status` output for the dsregcmd parser."
    },
    {
      "path": "evidence/event-logs/system.txt",
      "kind": "event-log-placeholder",
      "description": "Placeholder: real .evtx not shipped; see file for rationale."
    }
  ]
}
JSON

# --- evidence/logs/ccm.log --------------------------------------------------
# Three lines in the CMTrace <![LOG[...]LOG]!> format. Matches the wasm
# smoke's existing expectations (component/thread/severity) so the canary
# continues to pass unchanged.
cat > "${STAGE}/evidence/logs/ccm.log" <<'LOG'
<![LOG[Starting CcmExec service]LOG]!><time="08:00:00.0000000" date="3-15-2026" component="CcmExec" context="" type="1" thread="100" file="">
<![LOG[Processing policy update]LOG]!><time="08:00:01.0000000" date="3-15-2026" component="PolicyAgent" context="" type="1" thread="101" file="">
<![LOG[Error: Failed to connect to management point]LOG]!><time="08:00:02.0000000" date="3-15-2026" component="CcmExec" context="" type="3" thread="100" file="">
LOG

# --- evidence/logs/cbs.log --------------------------------------------------
# CBS servicing log format: `<Date> <Time>, <Level> <Source> <Message>`.
# Matches cmtraceopen/src-tauri/tests/corpus/cbs/clean/CBS.log shape so the
# Rust CBS parser accepts it without warnings.
cat > "${STAGE}/evidence/logs/cbs.log" <<'LOG'
2026-01-01 00:00:00, Info                  CBS    Exec: Processing package
2026-01-01 00:00:01, Warning               CSI    [SR] Verify staged payload
2026-01-01 00:00:02, Error                 CBS    Failed to apply update
LOG

# --- evidence/logs/panther.log ----------------------------------------------
# Panther setupact/setuperr format: `<Date> <Time>, <Level> <Source> <Msg>`
# with the `[0x...]` hex error-code token optional. Matches
# cmtraceopen/src-tauri/tests/corpus/panther/clean/setupact.log shape.
cat > "${STAGE}/evidence/logs/panther.log" <<'LOG'
2026-01-01 00:00:00, Info [0x080489] MIG Gather started
2026-01-01 00:00:01, Warning SP Retry required
2026-01-01 00:00:02, Error SP Setup rolled back
LOG

# --- evidence/logs/plain.log ------------------------------------------------
# Free-form lines that should trip no structured parser; the detector falls
# back to the plain-text parser. Mirrors the shape of
# cmtraceopen/src-tauri/tests/corpus/plain/unstructured.txt.
cat > "${STAGE}/evidence/logs/plain.log" <<'LOG'
This is a plain text log file with no structured format.
It should fall back to the plain text parser.
Final line of the fixture.
LOG

# --- evidence/command-output/dsregcmd-status.txt ----------------------------
# Synthetic `dsregcmd /status` output. Field names and section banners
# mirror the real command so the dsregcmd parser (see
# cmtraceopen/crates/cmtraceopen-parser/src/dsregcmd/parser.rs) lights up:
#   AzureAdJoined, DomainJoined, WorkplaceJoined, TenantId, DeviceId,
#   AzureAdPrt, MdmUrl, User State, SSO State.
# Values are obviously fake (all-zeros / all-ones GUIDs) to avoid
# exfiltration-shape concerns.
cat > "${STAGE}/evidence/command-output/dsregcmd-status.txt" <<'TXT'
+----------------------------------------------------------------------+
| Device State                                                         |
+----------------------------------------------------------------------+

             AzureAdJoined : YES
          EnterpriseJoined : NO
              DomainJoined : NO
                  TenantId : 00000000-0000-0000-0000-000000000001
                TenantName : Contoso Fixture Tenant
                  DeviceId : 00000000-0000-0000-0000-000000000002
                    Thumbprint : 0000000000000000000000000000000000000000
 DeviceCertificateValidity : [ 2026-01-01 00:00:00.000 UTC -- 2027-01-01 00:00:00.000 UTC ]
            KeyContainerId : 00000000-0000-0000-0000-000000000003
               KeyProvider : Microsoft Platform Crypto Provider
              TpmProtected : YES
        DeviceAuthStatus : SUCCESS

+----------------------------------------------------------------------+
| User State                                                           |
+----------------------------------------------------------------------+

                NgcSet : YES
             NgcKeyId : {00000000-0000-0000-0000-000000000004}
      CanReachDRS : YES
         WamDefaultSet : YES
       WamDefaultAuthority : organizations
             AzureAdPrt : YES
   AzureAdPrtUpdateTime : 2026-01-01 00:00:00.000 UTC
   AzureAdPrtExpiryTime : 2026-01-15 00:00:00.000 UTC
      AzureAdPrtAuthority : https://login.microsoftonline.com/00000000-0000-0000-0000-000000000001
               EnterprisePrt : NO
    EnterprisePrtAuthority :

+----------------------------------------------------------------------+
| SSO State                                                            |
+----------------------------------------------------------------------+

                 AzureAdPrt : YES
       AzureAdPrtUpdateTime : 2026-01-01 00:00:00.000 UTC
       AzureAdPrtExpiryTime : 2026-01-15 00:00:00.000 UTC
                 OnPremTgt : NO
                  CloudTgt : YES
      KerbTopLevelNames :

+----------------------------------------------------------------------+
| Diagnostic Data                                                      |
+----------------------------------------------------------------------+

                  AadRecoveryNeeded : NO
               Executing Account Name : SYSTEM
          KeySignTest : PASSED
         AD Connectivity Test : N/A
        DRS Discovery Test : N/A
                 User Context : SYSTEM
           Client ErrorCode : 0x0
TXT

# --- evidence/event-logs/system.txt -----------------------------------------
# Why a .txt placeholder and not real .evtx?
#   * Even a single-record System .evtx is ~32 KiB, which would blow this
#     fixture's <5 KiB budget by an order of magnitude.
#   * EVTX output is not byte-reproducible: the format embeds file-creation
#     timestamps, channel GUIDs, and chunk headers that vary per wevtutil /
#     PowerShell invocation, so we couldn't pin a deterministic sha256.
#   * In production, the Windows agent attaches the raw .evtx AND a
#     pre-parsed `analysis-input/*.json` sibling (same records, flattened,
#     deterministic). The web/wasm analyzer consumes the JSON; the .evtx is
#     retained for desktop drill-down. This placeholder documents that
#     contract so contributors don't wonder why event-logs/ is empty.
cat > "${STAGE}/evidence/event-logs/system.txt" <<'TXT'
PLACEHOLDER — real System.evtx is not shipped in this fixture.

Rationale:
  * A minimal EVTX record is ~32 KiB; this bundle budgets <5 KiB total.
  * EVTX headers embed creation timestamps / chunk GUIDs that can't be
    pinned deterministically, so the zip's sha256 would drift per build.

Production contract:
  * The Windows collector writes raw .evtx under evidence/event-logs/ AND
    emits a flattened, deterministic JSON sibling under
    analysis-input/event-logs/<channel>.json for each channel it snapshots
    (System, Application, Microsoft-Windows-User Device Registration, ...).
  * The web / wasm analyzer consumes the JSON sibling; the raw .evtx is
    surfaced only in the desktop client for drill-down.

If you need to exercise the event-log analyzer in tests, generate a JSON
sibling with a handful of records and wire it in alongside this file.
TXT

# Pin mtimes so zip headers are deterministic.
#
# GNU touch accepts `-d "@<epoch>"`, but macOS / BSD touch does not —
# it wants `-t YYYYMMDDhhmm.ss`. Detect the flavor once and wrap the
# per-file call so the script is portable between Linux CI, macOS dev
# boxes, and Git Bash on Windows (which ships GNU coreutils).
set_epoch_mtime() {
  if touch --version 2>/dev/null | grep -q GNU; then
    touch -d "@${SOURCE_DATE_EPOCH}" "$@"
  else
    local formatted
    formatted=$(date -r "${SOURCE_DATE_EPOCH}" +%Y%m%d%H%M.%S 2>/dev/null)
    touch -t "${formatted}" "$@"
  fi
}
export -f set_epoch_mtime
export SOURCE_DATE_EPOCH
find "${STAGE}" -exec bash -c 'set_epoch_mtime "$@"' _ {} +

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

# Sanity: keep the bundle tiny. 5 KiB leaves comfortable headroom for
# adding one or two more small fixtures without needing another review
# of this budget.
if [ "${SIZE}" -gt 5120 ]; then
  echo "error: test-bundle.zip is ${SIZE} bytes, exceeds 5120 byte budget." >&2
  echo "       either trim content or bump the budget intentionally in build.sh." >&2
  exit 1
fi

echo "built: ${OUT}"
echo "size : ${SIZE} bytes"
echo "sha256: ${SHA}"
