#!/usr/bin/env bash
# blob-backup.sh — Blob-store backup for CMTrace Open
#
# Syncs the local blob directory to either:
#   • Azure Blob Storage via azcopy sync   (BACKEND=azure)
#   • A local / remote directory via rsync  (BACKEND=local, default)
#
# Usage:
#   blob-backup.sh [options]
#
# Options:
#   --backend <azure|local>   Storage backend   (env: BACKEND,       default: local)
#   --src     <path>          Source blob dir    (env: BLOB_SRC,      default: /var/lib/cmtraceopen/data/blobs)
#   --dst     <url|path>      Destination        (env: BLOB_DST,      required)
#   --log     <logfile>       Log file           (env: LOG_FILE,      default: /var/log/cmtraceopen-blob-backup.log)
#   --dry-run                 Print what would happen, make no changes
#   --help                    Show this help and exit
#
# Azure backend:
#   BLOB_DST must be an Azure Blob Storage URL, e.g.:
#     https://<account>.blob.core.windows.net/<container>/blobs/
#   Authentication: set AZCOPY_AUTO_LOGIN_TYPE or log in via `azcopy login`
#   before running this script, or supply a SAS token in the URL.
#
# Local / rsync backend:
#   BLOB_DST is an rsync-compatible destination, e.g.:
#     /mnt/backup/cmtraceopen/blobs
#     backup-host:/backup/cmtraceopen/blobs
#
# MinIO (S3-compat) testing:
#   Export standard AWS env vars and use the local backend pointing at a
#   MinIO-backed mount, or set BACKEND=azure and use an azcopy-compatible
#   MinIO S3 gateway URL (azcopy supports S3 sources/destinations).
#   Alternatively, mount a MinIO-backed bucket with s3fs and use BACKEND=local.
#
# Exit codes:
#   0  — success
#   1  — sync failed
#   2  — bad arguments

set -euo pipefail

# ---------------------------------------------------------------------------
# defaults
# ---------------------------------------------------------------------------
BACKEND="${BACKEND:-local}"
BLOB_SRC="${BLOB_SRC:-/var/lib/cmtraceopen/data/blobs}"
BLOB_DST="${BLOB_DST:-}"
LOG_FILE="${LOG_FILE:-/var/log/cmtraceopen-blob-backup.log}"
DRY_RUN=false

# ---------------------------------------------------------------------------
# argument parsing
# ---------------------------------------------------------------------------
usage() {
  grep '^#' "$0" | grep -v '^#!/' | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --backend) BACKEND="$2";  shift 2 ;;
    --src)     BLOB_SRC="$2"; shift 2 ;;
    --dst)     BLOB_DST="$2"; shift 2 ;;
    --log)     LOG_FILE="$2"; shift 2 ;;
    --dry-run) DRY_RUN=true;  shift ;;
    --help)    usage 0 ;;
    *) echo "unknown option: $1" >&2; usage 2 ;;
  esac
done

if [[ -z "${BLOB_DST}" ]]; then
  echo "error: --dst / BLOB_DST is required" >&2
  usage 2
fi

case "${BACKEND}" in
  azure|local) ;;
  *) echo "error: unknown backend '${BACKEND}' (must be azure or local)" >&2; exit 2 ;;
esac

# ---------------------------------------------------------------------------
# logging helpers
# ---------------------------------------------------------------------------
log() {
  local ts
  ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  printf '[%s] %s\n' "${ts}" "$*" | tee -a "${LOG_FILE}"
}

die() {
  log "ERROR: $*"
  exit 1
}

# ---------------------------------------------------------------------------
# pre-flight
# ---------------------------------------------------------------------------
[[ -d "${BLOB_SRC}" ]] || die "Source directory not found: ${BLOB_SRC}"

# ---------------------------------------------------------------------------
# sync
# ---------------------------------------------------------------------------
log "Blob backup starting"
log "  backend : ${BACKEND}"
log "  src     : ${BLOB_SRC}"
log "  dst     : ${BLOB_DST}"
[[ "${DRY_RUN}" == "true" ]] && log "  mode    : DRY RUN"

case "${BACKEND}" in
  # -------------------------------------------------------------------------
  azure)
    command -v azcopy >/dev/null 2>&1 || die "azcopy not found on PATH. See https://learn.microsoft.com/en-us/azure/storage/common/storage-use-azcopy-v10 for installation."

    AZCOPY_ARGS=(
      sync
      "${BLOB_SRC}/"
      "${BLOB_DST}"
      --recursive
      --delete-destination=false
    )
    [[ "${DRY_RUN}" == "true" ]] && AZCOPY_ARGS+=(--dry-run)

    log "Running: azcopy ${AZCOPY_ARGS[*]}"
    if azcopy "${AZCOPY_ARGS[@]}"; then
      log "azcopy sync completed successfully"
    else
      die "azcopy sync failed"
    fi
    ;;

  # -------------------------------------------------------------------------
  local)
    command -v rsync >/dev/null 2>&1 || die "rsync not found on PATH"

    RSYNC_ARGS=(
      -avz
      --delete
      "${BLOB_SRC}/"
      "${BLOB_DST}/"
    )
    [[ "${DRY_RUN}" == "true" ]] && RSYNC_ARGS+=(--dry-run)

    log "Running: rsync ${RSYNC_ARGS[*]}"
    if rsync "${RSYNC_ARGS[@]}"; then
      log "rsync completed successfully"
    else
      die "rsync failed"
    fi
    ;;
esac

log "Blob backup finished successfully"
