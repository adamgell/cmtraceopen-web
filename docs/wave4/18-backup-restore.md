# 18 — Backup & Restore Runbook

**Applies to:** CMTrace Open — all deployment variants (BigMac26 self-hosted, Docker Compose, Azure Container Apps)

Cross-references:
- `docs/wave4/04-day2-operations.md` §5 (Disaster Recovery) + §7 (Backup + retention policy)
- `tools/ops/` — automation scripts

---

## Table of contents

1. [Overview](#1-overview)
2. [What is backed up](#2-what-is-backed-up)
3. [Retention policy](#3-retention-policy)
4. [Scheduling (cron)](#4-scheduling-cron)
5. [Postgres backup](#5-postgres-backup)
6. [Blob-store backup](#6-blob-store-backup)
7. [Restore procedures](#7-restore-procedures)
8. [Restore drill](#8-restore-drill)
9. [Azure managed-Postgres config](#9-azure-managed-postgres-config)
10. [What to do if a restore fails](#10-what-to-do-if-a-restore-fails)
11. [Drill log](#11-drill-log)

---

## 1. Overview

The backup strategy follows a **two-tier** model:

| Tier | What | Cadence | Tool |
|---|---|---|---|
| Postgres metadata | Single-DB `pg_dump` compressed dump | Weekly (Sunday 02:00 UTC) | `tools/ops/pg-backup.sh` |
| Blob store | rsync / azcopy sync (append-only) to secondary storage | Daily (03:00 UTC) | `tools/ops/blob-backup.sh` |

Restore drills are performed **quarterly**. The authoritative cadence and
drill calendar live in [`docs/wave4/20-dr-rehearsal.md`](20-dr-rehearsal.md);
this runbook reuses that schedule and does not redefine it. See §8 below
for the drill procedure (which is independent of cadence).

---

## 2. What is backed up

### 2.1 Postgres

- A **single application database** (default: `cmtrace`) via `pg_dump`
- Includes that database's schema + data only — **no cluster globals**
  (no `CREATE ROLE`, `ALTER ROLE`, `GRANT`, `CREATE TABLESPACE`, etc.)
- Cluster-wide objects (roles, tablespaces, role-level grants) are
  managed by Terraform / IaC and are deliberately NOT in the backup
  scope. This prevents a backup-restore cycle from silently overwriting
  production role passwords or grant matrices on a shared cluster.
- Output: a single `.sql.gz` file per run, named `cmtrace-<UTC-ts>.sql.gz`

### 2.2 Blob store

- The raw blob directory: `/var/lib/cmtraceopen/data/blobs` (self-hosted) or the mapped volume equivalent
- Contains uploaded evidence ZIPs, agent bundles, and other binary assets
- The sync is incremental (only changed/new files are transferred on subsequent runs)

### 2.3 Out of scope (deferred)

- WAL archiving / point-in-time recovery (PITR) — use Azure Flexible Server's built-in PITR for the cloud path
- Cross-region replication — Azure Storage GRS handles this when Azure backend is active
- Long-term cold archival — defer to Azure Archive tier

---

## 3. Retention policy

| Data | Retention | Pruning |
|---|---|---|
| Postgres dumps (`.sql.gz`) | 30 days | `pg-backup.sh` prunes automatically on each run |
| Blob store snapshots | 30 days | **Append-only** sync — destination expiry is handled by Azure Blob lifecycle policy (cloud) or the bundle TTL sweeper from PR #71 + NAS-side retention (self-hosted). The script never deletes files. |
| Azure Flexible Server automated backups | 30 days | Set via `backup_retention_days = 30` in Terraform |
| Azure geo-redundant backup | Enabled | Set via `geo_redundant_backup_enabled = true` |

---

## 4. Scheduling (cron)

### 4.1 Credentials — use `~/.pgpass`, not `PGPASSWORD` in cron

Embedding `PGPASSWORD=<secret>` directly in the crontab leaks the secret
two ways:

1. The crontab file itself is readable by the service account (and on
   some distros, by other users via `/var/spool/cron/`).
2. The expanded environment is visible in `ps -ef` for the duration of
   each `pg_dump` invocation.

Use libpq's `~/.pgpass` file instead. It must be `chmod 600` and live in
the home directory of the user running cron.

```bash
# As the cmtrace user
umask 077
cat > ~/.pgpass <<'EOF'
# hostname:port:database:username:password
localhost:5432:cmtrace:cmtrace:REPLACE_WITH_REAL_PASSWORD
localhost:5432:postgres:cmtrace:REPLACE_WITH_REAL_PASSWORD
EOF
chmod 600 ~/.pgpass
```

Verify libpq picks it up:

```bash
psql -h localhost -U cmtrace -d cmtrace -c 'SELECT 1;'   # should not prompt
```

### 4.2 Crontab entries

Add to the service account crontab (`crontab -e` as user `cmtrace`):

```cron
# --- CMTrace Open backup jobs ---
# Credentials come from ~/.pgpass (chmod 600). Do NOT add PGPASSWORD here.

# Postgres single-DB dump — weekly on Sunday at 02:00 UTC
0 2 * * 0  /srv/cmtraceopen/tools/ops/pg-backup.sh \
             -h localhost -U cmtrace -D cmtrace \
             -d /backup/cmtraceopen \
             -r 30 \
             >> /var/log/cmtraceopen-backup.log 2>&1

# Blob-store sync — daily at 03:00 UTC (append-only, no destination deletion)
0 3 * * *   BLOB_DST=/mnt/nas/blob-backups \
             /srv/cmtraceopen/tools/ops/blob-backup.sh \
             >> /var/log/cmtraceopen-blob-backup.log 2>&1
```

Verify the scripts are executable:

```bash
chmod +x /srv/cmtraceopen/tools/ops/pg-backup.sh
chmod +x /srv/cmtraceopen/tools/ops/pg-restore.sh
chmod +x /srv/cmtraceopen/tools/ops/blob-backup.sh
```

Confirm cron ran:

```bash
tail -50 /var/log/cmtraceopen-backup.log
tail -50 /var/log/cmtraceopen-blob-backup.log
```

Check that the dump file exists and is non-zero:

```bash
ls -lh /backup/cmtraceopen/ | grep "$(date +%Y)" | tail -5
```

---

## 5. Postgres backup

### 5.1 How it works

`tools/ops/pg-backup.sh` runs `pg_dump` against a **single application
database** (default: `cmtrace`) and pipes the output directly into
`gzip -9`. No uncompressed intermediate file is written, reducing disk
pressure.

The script uses `--no-owner --no-privileges`, so the dump can be replayed
into any cluster (including a fresh one) without depending on specific
role names existing.

Cluster globals (roles, tablespaces, GRANTs) are intentionally excluded —
they are managed by Terraform and must not be overwritten by a backup
pipeline. See §2.1 for the rationale.

Output filename format: `<dbname>-YYYY-MM-DDTHHMMSSZ.sql.gz`
(e.g. `cmtrace-2026-04-20T020000Z.sql.gz`).

### 5.2 Running manually

Credentials come from `~/.pgpass` (see §4.1).

```bash
# Self-hosted (BigMac26 / Docker Compose)
/srv/cmtraceopen/tools/ops/pg-backup.sh \
  -h localhost \
  -U cmtrace \
  -D cmtrace \
  -d /backup/cmtraceopen \
  -r 30

# Remote Postgres
/srv/cmtraceopen/tools/ops/pg-backup.sh \
  -h db.internal \
  -p 5432 \
  -U cmtrace \
  -D cmtrace \
  -d /backup/cmtraceopen
```

### 5.3 Verifying the output

```bash
# List recent dumps
ls -lh /backup/cmtraceopen/*.sql.gz | tail -5

# Peek at the dump header without full decompression
gunzip -c /backup/cmtraceopen/cmtrace-2026-04-20T020000Z.sql.gz | head -20
```

---

## 6. Blob-store backup

### 6.1 rsync (local / self-hosted)

```bash
BLOB_SRC=/var/lib/cmtraceopen/data/blobs \
BLOB_DST=/mnt/nas/blob-backups \
/srv/cmtraceopen/tools/ops/blob-backup.sh
```

### 6.2 Azure Blob Storage backend

```bash
# Authenticate first (interactive — for manual runs)
azcopy login

# Or use a SAS token embedded in the destination URL:
BACKEND=azure \
BLOB_SRC=/var/lib/cmtraceopen/data/blobs \
BLOB_DST="https://mystorageacct.blob.core.windows.net/cmtrace-backups/blobs/" \
/srv/cmtraceopen/tools/ops/blob-backup.sh
```

For unattended (cron) runs, configure a managed identity or a service principal with `Storage Blob Data Contributor` on the target container, then set:

```bash
export AZCOPY_AUTO_LOGIN_TYPE=MSI   # or AZCOPY_SPA_APPLICATION_ID + AZCOPY_SPA_CLIENT_SECRET
```

### 6.3 MinIO (local S3-compatible testing)

For local-repro testing against MinIO:

```bash
# Option A — mount MinIO bucket with s3fs, then use local backend
s3fs cmtrace-backups /mnt/minio-backups \
  -o url=http://minio.local:9000 \
  -o use_path_request_style \
  -o passwd_file=~/.passwd-s3fs

BLOB_DST=/mnt/minio-backups/blobs \
/srv/cmtraceopen/tools/ops/blob-backup.sh

# Option B — use azcopy with the MinIO S3 gateway
BACKEND=azure \
BLOB_DST="https://minio.local:9000/cmtrace-backups/blobs/?<sas-or-key>" \
/srv/cmtraceopen/tools/ops/blob-backup.sh
```

---

## 7. Restore procedures

### 7.1 Postgres restore (drill / verification)

Use `tools/ops/pg-restore.sh`. This script **always** restores into a
scratch database (`cmtrace_restore_test` by default) and explicitly
refuses to restore into `cmtrace`, `cmtraceopen`, or `postgres`.

The script expects single-DB `pg_dump` output (the format produced by
`pg-backup.sh` from PR #96 onward). It does **not** sanitise legacy
`pg_dumpall` dumps — restore those into a throwaway Postgres instance
(e.g. a Docker container) instead.

```bash
# Restore the most recent dump (credentials via ~/.pgpass)
LATEST=$(ls -t /backup/cmtraceopen/*.sql.gz | head -1)
/srv/cmtraceopen/tools/ops/pg-restore.sh "${LATEST}"

# Custom scratch DB name
/srv/cmtraceopen/tools/ops/pg-restore.sh \
  "${LATEST}" \
  -h localhost \
  -U cmtrace \
  -T drill_$(date +%Y%m%d)
```

Expected output:

```
[2026-04-20T03:00:12Z] Restore drill starting
[2026-04-20T03:00:12Z]   backup file : /backup/cmtraceopen/2026-04-20T020000Z.sql.gz
[2026-04-20T03:00:12Z]   target db   : cmtrace_restore_test
[2026-04-20T03:00:13Z] Dropping existing 'cmtrace_restore_test' (if any)
[2026-04-20T03:00:13Z] Creating scratch database 'cmtrace_restore_test'
[2026-04-20T03:00:13Z] Decompressing and restoring dump…
[2026-04-20T03:00:18Z] Restore completed
[2026-04-20T03:00:18Z] Running schema sanity check…
[2026-04-20T03:00:18Z]   Tables found: 12

============================================================
  RESTORE DRILL RESULT
============================================================
  backup file : /backup/cmtraceopen/2026-04-20T020000Z.sql.gz
  target db   : cmtrace_restore_test
  tables      : 12
  result      : PASS
============================================================
```

After the drill, drop the scratch DB to reclaim space:

```bash
psql -U postgres -c 'DROP DATABASE IF EXISTS cmtrace_restore_test;'
```

### 7.2 Postgres restore into production (disaster recovery)

> Only perform this in a declared disaster-recovery event. Requires downtime.

This procedure replays a single-DB `pg_dump` archive into a freshly
created `cmtrace` database. It assumes:

- Cluster globals (roles, tablespaces) already exist (managed by Terraform / IaC).
- The Postgres instance is running but the cmtraceopen application stack is stopped.

```bash
# 1. Stop the stack
cd /srv/cmtraceopen && docker compose down

# 2. Identify the latest good backup
ls -lht /backup/cmtraceopen/*.sql.gz | head -5
BACKUP=/backup/cmtraceopen/cmtrace-YYYY-MM-DDTHHMMSSZ.sql.gz   # pick one

# 3. Drop and recreate the cmtrace database.
#    Credentials come from ~/.pgpass for the postgres superuser.
#    DO NOT skip this step — replaying a dump on top of an existing
#    (possibly half-corrupt) DB can leave a worse state than starting fresh.
psql -h localhost -U postgres -d postgres <<'SQL'
DROP DATABASE IF EXISTS cmtrace;
CREATE DATABASE cmtrace OWNER cmtrace;
SQL

# 4. Restore the dump into the fresh cmtrace database.
#    --set ON_ERROR_STOP=1 makes psql exit on the first SQL error so
#    a partial restore does not silently produce a corrupt DB.
gunzip -c "${BACKUP}" \
  | psql -h localhost -U cmtrace -d cmtrace \
      --set ON_ERROR_STOP=1 -v ON_ERROR_STOP=1

# 5. Restart the stack
docker compose up -d

# 6. Verify
curl -sf http://localhost:8080/healthz
```

### 7.3 Blob-store restore

```bash
# Restore blobs from NAS backup to local path
rsync -avz /mnt/nas/blob-backups/ /var/lib/cmtraceopen/data/blobs/

# From Azure Blob Storage
azcopy sync \
  "https://mystorageacct.blob.core.windows.net/cmtrace-backups/blobs/" \
  "/var/lib/cmtraceopen/data/blobs/" \
  --recursive
```

---

## 8. Restore drill

### 8.1 Cadence

**Quarterly** (once per calendar quarter). The authoritative drill
calendar — including specific drill dates and the Q1–Q4 scenario
rotation — lives in
[`docs/wave4/20-dr-rehearsal.md`](20-dr-rehearsal.md). This runbook
covers only the **Postgres restore drill**, which is one of the four
quarterly scenarios; refer to the DR rehearsal doc for cadence,
rotation, and post-mortem requirements.

### 8.2 Drill procedure

1. **Identify the backup under test** — use the most recent weekly Sunday dump.

   ```bash
   LATEST=$(ls -t /backup/cmtraceopen/*.sql.gz | head -1)
   echo "Testing: ${LATEST}"
   ```

2. **Run the restore script** against the scratch DB (credentials via `~/.pgpass`):

   ```bash
   /srv/cmtraceopen/tools/ops/pg-restore.sh "${LATEST}"
   ```

3. **Validate schema** — confirm the expected tables are present:

   ```bash
   psql -U postgres -d cmtrace_restore_test -c "\dt *.*"
   ```

4. **Validate a row count** — compare against production:

   ```bash
   # Production
   psql -U cmtrace -d cmtrace -c 'SELECT count(*) FROM sessions;'

   # Restored (should be ≤ prod, not wildly off)
   psql -U postgres -d cmtrace_restore_test -c 'SELECT count(*) FROM sessions;'
   ```

5. **Record the result** in the [Drill log](#11-drill-log) below.

6. **Drop the scratch DB:**

   ```bash
   psql -U postgres -c 'DROP DATABASE IF EXISTS cmtrace_restore_test;'
   ```

### 8.3 Pass criteria

- Restore script exits 0
- At least one table found in the restored schema
- `sessions` row count ≥ 0 (even 0 is acceptable for a fresh install backup)
- Wall-clock restore time < 15 minutes

---

## 9. Azure managed-Postgres config

When deploying to Azure Flexible Server, the following Terraform settings in
`infra/azure/modules/postgres/main.tf` must be set:

```hcl
resource "azurerm_postgresql_flexible_server" "main" {
  # … other config …

  backup_retention_days        = 30
  geo_redundant_backup_enabled = true
}
```

Azure's managed backup provides:
- Automated daily full backups + transaction log backups
- Point-in-time restore (PITR) up to `backup_retention_days`
- Geo-redundant storage copies data to a paired region

The `pg-backup.sh` script provides an **additional** application-level dump
that is independent of the Azure managed backup — this is the "belt and
suspenders" layer that also works in the self-hosted path.

---

## 10. What to do if a restore fails

### Restore script errors

| Error | Likely cause | Action |
|---|---|---|
| `pg_dumpall not found` | PostgreSQL client tools not installed | Install `postgresql-client` package |
| `psql: error: connection refused` | Postgres not running / wrong host | Check `PG_HOST`, `PG_PORT`; ensure Postgres is up |
| `password authentication failed` | Wrong `PGPASSWORD` | Check credentials; try `psql -U <user>` interactively |
| Output file is empty | `pg_dumpall` failed silently | Run `pg_dumpall` manually to see error; check disk space |
| `No tables found` warning | Empty or schema-only dump | Inspect the dump: `gunzip -c <file> | head -100` |
| Restore completes but row counts are wrong | Backup is from wrong point in time | Identify the last known-good dump; re-run drill |

### Escalation path

1. Check the log file: `tail -100 /var/log/cmtraceopen-backup.log`
2. Verify disk space: `df -h /backup/cmtraceopen`
3. Run `pg_dumpall` manually to isolate the error:
   ```bash
   PGPASSWORD=secret pg_dumpall -h localhost -U cmtrace | head -50
   ```
4. If the most recent backup is corrupt, fall back to the previous week's dump (there should be ≥ 4 weekly dumps in the 30-day window).
5. If all backups in the 30-day window are suspect, restore from the Azure managed backup (PITR) for the cloud path, or contact the team.

---

## 11. Drill log

Document each quarterly drill here. Keep the most recent 8 entries (2 years).

| Date | Backup tested | Tables found | Row count (sessions) | Wall-clock time | Result | Performed by | Notes |
|---|---|---|---|---|---|---|---|
| *(pending — do first drill)* | | | | | | | |
