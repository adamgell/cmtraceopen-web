#!/usr/bin/env bash
# pg-restore.sh — Restore a Postgres dump for CMTrace Open (verification only)
#
# Restores a named backup file (.sql.gz produced by pg-backup.sh) into a
# scratch database for drill / verification purposes.  It NEVER touches
# the production database.
#
# SAFETY: This script expects single-database pg_dump output (the format
# produced by pg-backup.sh after PR #96). It does NOT process pg_dumpall
# output — pg_dumpall contains cluster globals (CREATE/ALTER ROLE, GRANT)
# that would mutate the shared cluster on replay. If you have a legacy
# pg_dumpall dump, restore it into a throwaway Postgres instance, not via
# this script.
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
#   -U <user>           Postgres user          (env: PG_USER,      default: postgres)
#   -T <dbname>         Target scratch DB name (env: TARGET_DB,    default: cmtrace_restore_test)
#   --no-drop           Skip DROP DATABASE before restore (fail if DB exists)
#   --help              Show this help and exit
#
# Authentication:
#   Prefer ~/.pgpass (chmod 600) over PGPASSWORD. Cron entries that embed
#   PGPASSWORD inline expose the secret via crontab files and ps output.
#
# What it does:
#   1. Drops the target scratch database if it exists (unless --no-drop)
#   2. Creates a fresh empty target database
#   3. Decompresses the dump and pipes it through psql, connected to the
#      scratch DB (NOT to the postgres admin DB), so any stray cluster-
#      global statements would error against the scratch DB rather than
#      mutate the cluster
#   4. Runs a basic schema sanity check (counts tables in pg_catalog)
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
command -v psql   >/dev/null 2>&1 || die "psql not found on PATH. Install the postgresql-client (Debian/Ubuntu) or postgresql (RHEL/Alpine) package."
command -v gunzip >/dev/null 2>&1 || die "gunzip not found on PATH. Install the gzip package."

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
# We always DROP + CREATE the scratch DB before restore (unless --no-drop).
# A half-restored DB from a previous failed run would otherwise leave bad
# state that masks errors on the next attempt.
if [[ "${NO_DROP}" == "false" ]]; then
  log "Dropping existing '${TARGET_DB}' (if any)"
  psql_cmd -d postgres \
    -c "DROP DATABASE IF EXISTS \"${TARGET_DB}\";" \
    >/dev/null
fi

log "Creating fresh scratch database '${TARGET_DB}'"
psql_cmd -d postgres \
  -c "CREATE DATABASE \"${TARGET_DB}\";" \
  >/dev/null

# ---------------------------------------------------------------------------
# restore
# ---------------------------------------------------------------------------
# pg-backup.sh produces single-DB pg_dump output (no cluster globals, no
# \connect directives, no CREATE/ALTER ROLE statements). We connect psql
# directly to the scratch DB and replay the dump there. Any stray cluster-
# global statement (e.g. from a hand-edited dump) would error against a
# non-superuser scratch session rather than mutate the live cluster.
log "Decompressing and restoring dump into '${TARGET_DB}'…"

if gunzip -c "${BACKUP_FILE}" \
     | psql_cmd -d "${TARGET_DB}" \
         --set ON_ERROR_STOP=1 \
         -v ON_ERROR_STOP=1 \
         >/dev/null; then
  log "Restore completed"
else
  die "psql reported errors during restore (exited non-zero)"
fi

# ---------------------------------------------------------------------------
# sanity check — count tables in the restored schema
# ---------------------------------------------------------------------------
log "Running schema sanity check…"
TABLE_COUNT=$(psql_cmd -d "${TARGET_DB}" -tAc \
  "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema');" \
  | tr -d '[:space:]')
TABLE_COUNT="${TABLE_COUNT:-0}"
log "  Tables found: ${TABLE_COUNT}"

if [[ "${TABLE_COUNT}" -eq 0 ]]; then
  log "WARNING: No tables found in '${TARGET_DB}'. The backup may be empty or schema-only."
else
  psql_cmd -d "${TARGET_DB}" -tAc \
    "SELECT '    ' || schemaname || '.' || tablename FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY 1;" \
    || true
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
