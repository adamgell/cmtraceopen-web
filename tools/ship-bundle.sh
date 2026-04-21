#!/usr/bin/env bash
# Reference ingest client for the CMTrace Open api-server.
#
# Speaks the bundle-ingest protocol defined in `crates/common-wire` on
# branch `feat/api-ingest-v0` (PR #7): init -> chunk* -> finalize.
#
# This is the same wire contract the Windows agent will implement. If you
# change the protocol, this script changes too -- they evolve together.
#
# Deps: bash 4+, curl, jq, sha256sum (or shasum), stat, head (dd for
# chunk slicing). No exotic tooling.

set -euo pipefail

# ---------------------------------------------------------------------------
# args
# ---------------------------------------------------------------------------
ENDPOINT="http://localhost:8080"
DEVICE_ID=""
BUNDLE=""
BUNDLE_ID=""
CONTENT_KIND="evidence-zip"

usage() {
  cat <<EOF
Usage: $0 --device-id <id> --bundle <path> [options]

Required:
  --device-id <string>    Value sent in X-Device-Id; acts as identity until mTLS M2.
  --bundle <path>         Path to the bundle file to upload (typically the
                          fixture zip produced by tools/fixtures/build.sh).

Options:
  --endpoint <url>        API base URL (default: ${ENDPOINT}).
  --bundle-id <uuid>      Stable bundle id; generated if omitted. Reuse the
                          same value to exercise the resume path.
  --content-kind <kind>   evidence-zip | ndjson-entries | raw-file
                          (default: ${CONTENT_KIND}).
  -h, --help              Show this help and exit.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --endpoint)     ENDPOINT="$2"; shift 2 ;;
    --device-id)    DEVICE_ID="$2"; shift 2 ;;
    --bundle)       BUNDLE="$2"; shift 2 ;;
    --bundle-id)    BUNDLE_ID="$2"; shift 2 ;;
    --content-kind) CONTENT_KIND="$2"; shift 2 ;;
    -h|--help)      usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "${DEVICE_ID}" || -z "${BUNDLE}" ]]; then
  echo "error: --device-id and --bundle are required" >&2
  usage >&2
  exit 2
fi

if [[ ! -f "${BUNDLE}" ]]; then
  echo "error: bundle not found: ${BUNDLE}" >&2
  exit 2
fi

for bin in curl jq; do
  command -v "${bin}" >/dev/null 2>&1 || {
    echo "error: required binary not on PATH: ${bin}" >&2
    exit 2
  }
done

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

# sha256 of a file, lowercase hex. sha256sum on Linux, shasum -a 256 on macOS.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# byte size of a file. stat flags differ across BSD/GNU.
size_of() {
  if stat -c%s "$1" >/dev/null 2>&1; then
    stat -c%s "$1"
  else
    stat -f%z "$1"
  fi
}

# uuid generator: uuidgen if present (macOS + many Linuxes), otherwise
# kernel entropy file. Lowercased.
make_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  elif [[ -r /proc/sys/kernel/random/uuid ]]; then
    cat /proc/sys/kernel/random/uuid
  else
    echo "error: no uuidgen and no /proc/sys/kernel/random/uuid; pass --bundle-id" >&2
    return 1
  fi
}

# Run curl, split HTTP status from body. Writes body to $1 (a path), echoes
# the status code on stdout. Extra args are passed to curl.
http_call() {
  local body_out="$1"; shift
  curl -sS -o "${body_out}" -w '%{http_code}' "$@"
}

fail() {
  local code="$1" body="$2" ctx="$3"
  echo "error: ${ctx} returned HTTP ${code}" >&2
  if [[ -s "${body}" ]]; then
    echo "response body:" >&2
    cat "${body}" >&2
    echo >&2
  fi
  exit 1
}

# ---------------------------------------------------------------------------
# compute fixture facts
# ---------------------------------------------------------------------------
if [[ -z "${BUNDLE_ID}" ]]; then
  BUNDLE_ID="$(make_uuid)"
fi

SIZE="$(size_of "${BUNDLE}")"
SHA="$(sha256_of "${BUNDLE}")"

echo "bundle    : ${BUNDLE}"
echo "size      : ${SIZE} bytes"
echo "sha256    : ${SHA}"
echo "bundle-id : ${BUNDLE_ID}"
echo "device-id : ${DEVICE_ID}"
echo "endpoint  : ${ENDPOINT}"
echo

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

# ---------------------------------------------------------------------------
# 1. init
# ---------------------------------------------------------------------------
INIT_REQ="${TMP}/init-req.json"
INIT_RES="${TMP}/init-res.json"
jq -n \
  --arg bid "${BUNDLE_ID}" \
  --arg dh  "${DEVICE_ID}" \
  --arg sha "${SHA}" \
  --argjson sz "${SIZE}" \
  --arg ck  "${CONTENT_KIND}" \
  '{bundleId:$bid, deviceHint:$dh, sha256:$sha, sizeBytes:$sz, contentKind:$ck}' \
  > "${INIT_REQ}"

echo "-> POST /v1/ingest/bundles"
CODE=$(http_call "${INIT_RES}" \
  -X POST "${ENDPOINT}/v1/ingest/bundles" \
  -H "Content-Type: application/json" \
  -H "X-Device-Id: ${DEVICE_ID}" \
  --data-binary "@${INIT_REQ}")

case "${CODE}" in
  200|201) ;;
  409) fail "${CODE}" "${INIT_RES}" "init (conflict)" ;;
  *)   fail "${CODE}" "${INIT_RES}" "init" ;;
esac

UPLOAD_ID=$(jq -r '.uploadId'     "${INIT_RES}")
CHUNK_SIZE=$(jq -r '.chunkSize'   "${INIT_RES}")
RESUME=$(jq -r '.resumeOffset'    "${INIT_RES}")

if [[ -z "${UPLOAD_ID}" || "${UPLOAD_ID}" == "null" ]]; then
  echo "error: init response missing uploadId" >&2
  cat "${INIT_RES}" >&2
  exit 1
fi

echo "   upload-id   : ${UPLOAD_ID}"
echo "   chunk-size  : ${CHUNK_SIZE} bytes"
echo "   resume-at   : ${RESUME}"

# If the server says the upload is already complete (resume == size), skip
# straight to finalize. This covers both "we already finalized" (init
# returned 200 with resume == size) and "every byte is already staged".
OFFSET="${RESUME}"

# ---------------------------------------------------------------------------
# 2. chunked upload
# ---------------------------------------------------------------------------
while (( OFFSET < SIZE )); do
  REMAINING=$(( SIZE - OFFSET ))
  THIS=$(( CHUNK_SIZE < REMAINING ? CHUNK_SIZE : REMAINING ))

  CHUNK="${TMP}/chunk.bin"
  # Portable byte-range slice. dd+bs=1 is too slow on big files; instead
  # we tail from OFFSET (+1 because tail is 1-indexed) and head the first
  # THIS bytes. Both honor binary bytes correctly when LC_ALL=C, and both
  # are in POSIX toolsets on macOS and Linux.
  LC_ALL=C tail -c +"$((OFFSET + 1))" "${BUNDLE}" | LC_ALL=C head -c "${THIS}" > "${CHUNK}"
  GOT=$(size_of "${CHUNK}")
  if [[ "${GOT}" != "${THIS}" ]]; then
    echo "error: chunk slice produced ${GOT} bytes, expected ${THIS}" >&2
    exit 1
  fi

  PCT=$(( (OFFSET + THIS) * 100 / SIZE ))
  printf -- "-> PUT chunk offset=%s len=%s  (%d%%)\n" "${OFFSET}" "${THIS}" "${PCT}"

  CHUNK_RES="${TMP}/chunk-res.json"
  CODE=$(http_call "${CHUNK_RES}" \
    -X PUT "${ENDPOINT}/v1/ingest/bundles/${UPLOAD_ID}/chunks?offset=${OFFSET}" \
    -H "Content-Type: application/octet-stream" \
    -H "X-Device-Id: ${DEVICE_ID}" \
    --data-binary "@${CHUNK}")

  if [[ "${CODE}" != "200" ]]; then
    fail "${CODE}" "${CHUNK_RES}" "chunk offset=${OFFSET}"
  fi

  NEXT=$(jq -r '.nextOffset' "${CHUNK_RES}")
  EXPECTED=$(( OFFSET + THIS ))
  if [[ "${NEXT}" != "${EXPECTED}" ]]; then
    echo "error: server returned nextOffset=${NEXT}, expected ${EXPECTED}" >&2
    exit 1
  fi
  OFFSET="${NEXT}"
done

echo "   upload complete: ${OFFSET}/${SIZE} bytes"

# ---------------------------------------------------------------------------
# 3. finalize
# ---------------------------------------------------------------------------
# For a completed upload the final sha is identical to the init sha. We
# re-assert rather than reuse the variable so the client-side hash is
# computed over the actual bytes on disk at finalize time.
FINAL_SHA="$(sha256_of "${BUNDLE}")"
if [[ "${FINAL_SHA}" != "${SHA}" ]]; then
  echo "error: bundle changed on disk mid-upload (sha drifted: ${SHA} -> ${FINAL_SHA})" >&2
  exit 1
fi

FIN_REQ="${TMP}/fin-req.json"
FIN_RES="${TMP}/fin-res.json"
jq -n --arg s "${FINAL_SHA}" '{finalSha256:$s}' > "${FIN_REQ}"

echo "-> POST /v1/ingest/bundles/${UPLOAD_ID}/finalize"
CODE=$(http_call "${FIN_RES}" \
  -X POST "${ENDPOINT}/v1/ingest/bundles/${UPLOAD_ID}/finalize" \
  -H "Content-Type: application/json" \
  -H "X-Device-Id: ${DEVICE_ID}" \
  --data-binary "@${FIN_REQ}")

case "${CODE}" in
  200|201) ;;
  *) fail "${CODE}" "${FIN_RES}" "finalize" ;;
esac

SESSION_ID=$(jq -r '.sessionId'  "${FIN_RES}")
PARSE=$(jq -r '.parseState'      "${FIN_RES}")

if [[ -z "${SESSION_ID}" || "${SESSION_ID}" == "null" ]]; then
  echo "error: finalize response missing sessionId" >&2
  cat "${FIN_RES}" >&2
  exit 1
fi

echo
printf "OK  device_id=%s  session_id=%s  bundle_id=%s  bytes=%s  parse_state=%s\n" \
  "${DEVICE_ID}" "${SESSION_ID}" "${BUNDLE_ID}" "${SIZE}" "${PARSE}"
