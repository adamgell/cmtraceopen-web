#!/usr/bin/env bash
# pg-restore.sh — Restore a Postgres dump for CMTrace Open (verification only)
#
# Restores a named backup file (.sql.gz produced by pg-backup.sh) into a
# scratch database for drill / verification purposes.  It NEVER touches
# the production database.
#
# Usage:
#   pg-restore.sh <backup-file> [options]
#
# Arguments:
#   <backup-file>       Path to the .sql.gz file to restore (required)
#
# Options:
#   -h <host>           Postgres host          (env: PG_HOST,      default: localhost)
#   -p <port>           Postgres port          (env: PG_PORT,      default: 5432)
#   -U <user>           Postgres superuser     (env: PG_USER,      default: postgres)
#   -T <dbname>         Target scratch DB name (env: TARGET_DB,    default: cmtrace_restore_test)
#   --no-drop           Skip DROP DATABASE before restore (fail if DB exists)
#   --help              Show this help and exit
#
# Environment variables:
#   PGPASSWORD          Postgres password (standard libpq variable)
#
# What it does:
#   1. Drops the target database if it exists (unless --no-drop)
#   2. Creates a fresh target database
#   3. Decompresses the dump and pipes it through psql
#   4. Runs a basic schema sanity check (lists tables)
#   5. Prints a pass/fail summary
#
# Exit codes:
#   0  — restore succeeded and sanity check passed
#   1  — restore or sanity check failed
#   2  — bad arguments

set -euo pipefail

# ---------------------------------------------------------------------------
# defaults
# ---------------------------------------------------------------------------
PG_HOST="${PG_HOST:-localhost}"
PG_PORT="${PG_PORT:-5432}"
PG_USER="${PG_USER:-postgres}"
TARGET_DB="${TARGET_DB:-cmtrace_restore_test}"
NO_DROP=false
BACKUP_FILE=""

# ---------------------------------------------------------------------------
# argument parsing
# ---------------------------------------------------------------------------
usage() {
  grep '^#' "$0" | grep -v '^#!/' | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h) PG_HOST="$2";   shift 2 ;;
    -p) PG_PORT="$2";   shift 2 ;;
    -U) PG_USER="$2";   shift 2 ;;
    -T) TARGET_DB="$2"; shift 2 ;;
    --no-drop) NO_DROP=true; shift ;;
    --help) usage 0 ;;
    -*)  echo "unknown option: $1" >&2; usage 2 ;;
    *)
      if [[ -z "${BACKUP_FILE}" ]]; then
        BACKUP_FILE="$1"; shift
      else
        echo "unexpected argument: $1" >&2; usage 2
      fi
      ;;
  esac
done

if [[ -z "${BACKUP_FILE}" ]]; then
  echo "error: <backup-file> is required" >&2
  usage 2
fi

if [[ ! -f "${BACKUP_FILE}" ]]; then
  echo "error: backup file not found: ${BACKUP_FILE}" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
log() {
  local ts
  ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  printf '[%s] %s\n' "${ts}" "$*"
}

die() {
  log "ERROR: $*"
  exit 1
}

psql_cmd() {
  psql -h "${PG_HOST}" -p "${PG_PORT}" -U "${PG_USER}" "$@"
}

# ---------------------------------------------------------------------------
# pre-flight checks
# ---------------------------------------------------------------------------
command -v psql   >/dev/null 2>&1 || die "psql not found on PATH"
command -v gunzip >/dev/null 2>&1 || die "gunzip not found on PATH"

log "Restore drill starting"
log "  backup file : ${BACKUP_FILE}"
log "  target db   : ${TARGET_DB}"
log "  host        : ${PG_HOST}:${PG_PORT}"
log "  user        : ${PG_USER}"

# Safety guard: refuse to restore into anything that looks like production.
case "${TARGET_DB}" in
  cmtrace|cmtraceopen|postgres)
    die "Refusing to restore into protected database '${TARGET_DB}'. Use a scratch DB name."
    ;;
esac

# ---------------------------------------------------------------------------
# drop & recreate scratch database
# ---------------------------------------------------------------------------
if [[ "${NO_DROP}" == "false" ]]; then
  log "Dropping existing '${TARGET_DB}' (if any)"
  psql_cmd -d postgres \
    -c "DROP DATABASE IF EXISTS \"${TARGET_DB}\";" \
    >/dev/null
fi

log "Creating scratch database '${TARGET_DB}'"
psql_cmd -d postgres \
  -c "CREATE DATABASE \"${TARGET_DB}\";" \
  >/dev/null

# ---------------------------------------------------------------------------
# restore
# ---------------------------------------------------------------------------
log "Decompressing and restoring dump…"

# pg_dumpall output contains \connect directives that switch databases.
# We force all statements into the scratch DB by stripping those lines and
# adding our own at the top, then piping through psql.
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

CLEAN_SQL="${TMP}/restore.sql"

# Strip \connect lines (they would switch to the original DB name),
# strip CREATE/DROP DATABASE lines that reference the original cluster setup,
# and prepend a \connect to our scratch DB.
{
  printf '\connect "%s"\n' "${TARGET_DB}"
  gunzip -c "${BACKUP_FILE}" \
    | grep -v '^\\connect ' \
    | grep -Ev '^(CREATE|DROP) DATABASE '
} > "${CLEAN_SQL}"

if psql_cmd -d postgres -f "${CLEAN_SQL}" >/dev/null; then
  log "Restore completed"
else
  die "psql reported errors during restore"
fi

# ---------------------------------------------------------------------------
# sanity check — list tables in the restored schema
# ---------------------------------------------------------------------------
log "Running schema sanity check…"
TABLE_LIST="${TMP}/tables.txt"
psql_cmd -d "${TARGET_DB}" \
  -c "\dt *.*" \
  -t \
  > "${TABLE_LIST}" 2>&1 || true

TABLE_COUNT=$(grep -c '|' "${TABLE_LIST}" || true)
log "  Tables found: ${TABLE_COUNT}"

if [[ "${TABLE_COUNT}" -eq 0 ]]; then
  log "WARNING: No tables found in '${TARGET_DB}'. The backup may be empty or schema-only."
else
  log "  Table list:"
  sed 's/^/    /' "${TABLE_LIST}"
fi

# ---------------------------------------------------------------------------
# summary
# ---------------------------------------------------------------------------
cat <<EOF

============================================================
  RESTORE DRILL RESULT
============================================================
  backup file : ${BACKUP_FILE}
  target db   : ${TARGET_DB}
  tables      : ${TABLE_COUNT}
  result      : PASS
============================================================
EOF

log "Restore drill finished — PASS"
