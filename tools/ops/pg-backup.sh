#!/usr/bin/env bash
# pg-backup.sh — Full Postgres dump for CMTrace Open
#
# Runs pg_dumpall, compresses the output with gzip, and prunes backups
# older than RETENTION_DAYS. Designed to run from cron (see README.md).
#
# Usage:
#   pg-backup.sh [options]
#
# Options:
#   -h <host>           Postgres host          (env: PG_HOST,    default: localhost)
#   -p <port>           Postgres port          (env: PG_PORT,    default: 5432)
#   -U <user>           Postgres superuser     (env: PG_USER,    default: postgres)
#   -d <dir>            Backup output dir      (env: BACKUP_DIR, default: /backup/cmtraceopen)
#   -r <days>           Retention in days      (env: RETENTION_DAYS, default: 30)
#   -l <logfile>        Log file path          (env: LOG_FILE,   default: /var/log/cmtraceopen-backup.log)
#   --help              Show this help and exit
#
# Environment variables (all overridable via CLI flags above):
#   PGPASSWORD          Postgres password (standard libpq variable)
#
# Exit codes:
#   0  — success
#   1  — backup failed
#   2  — bad arguments

set -euo pipefail

# ---------------------------------------------------------------------------
# defaults (overridable via env or CLI)
# ---------------------------------------------------------------------------
PG_HOST="${PG_HOST:-localhost}"
PG_PORT="${PG_PORT:-5432}"
PG_USER="${PG_USER:-postgres}"
BACKUP_DIR="${BACKUP_DIR:-/backup/cmtraceopen}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
LOG_FILE="${LOG_FILE:-/var/log/cmtraceopen-backup.log}"

# ---------------------------------------------------------------------------
# argument parsing
# ---------------------------------------------------------------------------
usage() {
  grep '^#' "$0" | grep -v '^#!/' | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h) PG_HOST="$2";       shift 2 ;;
    -p) PG_PORT="$2";       shift 2 ;;
    -U) PG_USER="$2";       shift 2 ;;
    -d) BACKUP_DIR="$2";    shift 2 ;;
    -r) RETENTION_DAYS="$2"; shift 2 ;;
    -l) LOG_FILE="$2";      shift 2 ;;
    --help) usage 0 ;;
    *) echo "unknown option: $1" >&2; usage 2 ;;
  esac
done

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
# pre-flight checks
# ---------------------------------------------------------------------------
command -v pg_dumpall >/dev/null 2>&1 || die "pg_dumpall not found on PATH. Install the postgresql-client (Debian/Ubuntu) or postgresql (RHEL/Alpine) package."
command -v gzip       >/dev/null 2>&1 || die "gzip not found on PATH. Install the gzip package."

mkdir -p "${BACKUP_DIR}" || die "cannot create backup directory: ${BACKUP_DIR}"

# ---------------------------------------------------------------------------
# backup
# ---------------------------------------------------------------------------
TS="$(date -u '+%Y-%m-%dT%H%M%SZ')"
OUT="${BACKUP_DIR}/${TS}.sql.gz"

log "Starting full Postgres dump → ${OUT}"
log "  host=${PG_HOST}  port=${PG_PORT}  user=${PG_USER}"

# pg_dumpall writes to stdout; we pipe directly into gzip so no
# uncompressed intermediate file is needed (avoids disk pressure).
if pg_dumpall \
      -h "${PG_HOST}" \
      -p "${PG_PORT}" \
      -U "${PG_USER}" \
    | gzip -9 > "${OUT}"; then
  SIZE=$(du -sh "${OUT}" | cut -f1)
  log "Dump complete: ${OUT} (${SIZE})"
else
  # Remove partial file on failure to avoid confusion
  rm -f "${OUT}"
  die "pg_dumpall failed — see log for details"
fi

# Sanity-check: the compressed file must be non-empty
[[ -s "${OUT}" ]] || die "Output file is empty: ${OUT}"

# ---------------------------------------------------------------------------
# retention pruning
# ---------------------------------------------------------------------------
log "Pruning backups older than ${RETENTION_DAYS} days in ${BACKUP_DIR}"
PRUNED=0
while IFS= read -r -d '' f; do
  log "  Removing old backup: ${f}"
  rm -f "${f}"
  (( PRUNED++ )) || true
done < <(find "${BACKUP_DIR}" -maxdepth 1 -name '*.sql.gz' \
           -mtime +"${RETENTION_DAYS}" -print0)

log "Pruned ${PRUNED} old backup(s)"
log "Backup job finished successfully"
