# tools/ops — CMTrace Open Operations Scripts

Day-2 operational scripts for Postgres and blob-store backup and restore.

## Scripts

| Script | Purpose |
|---|---|
| [`pg-backup.sh`](pg-backup.sh) | Single-DB `pg_dump` → gzip, with automatic pruning of old backups (no cluster globals) |
| [`pg-restore.sh`](pg-restore.sh) | Restore a named backup file into a scratch database (verification / drill) |
| [`blob-backup.sh`](blob-backup.sh) | **Append-only** sync of the blob store to Azure Blob Storage (`azcopy`) or a remote/local path (`rsync`); never deletes destination files |

For the full runbook, retention policy, and restore drill guidance see
[`docs/wave4/18-backup-restore.md`](../../docs/wave4/18-backup-restore.md).

---

## Quick start

### Credentials — use `~/.pgpass`

Both Postgres scripts read passwords from libpq's `~/.pgpass` file (mode
`0600`) by default. Avoid setting `PGPASSWORD` inline on the command
line or in cron — it leaks via crontab files and `ps -ef` output.

```bash
# As the user that runs the scripts
umask 077
cat > ~/.pgpass <<'EOF'
# hostname:port:database:username:password
localhost:5432:cmtrace:cmtrace:REPLACE_ME
localhost:5432:postgres:cmtrace:REPLACE_ME
EOF
chmod 600 ~/.pgpass
```

### pg-backup.sh

```bash
# Minimal — Postgres on localhost, default user, default DB (cmtrace)
./pg-backup.sh

# Custom host / user / database / backup dir
./pg-backup.sh \
  -h db.internal \
  -U cmtrace \
  -D cmtrace \
  -d /mnt/nas/pg-backups \
  -r 30
```

### pg-restore.sh

```bash
# Restore the most recent backup into the default scratch DB
./pg-restore.sh /backup/cmtraceopen/cmtrace-2026-04-20T020000Z.sql.gz

# Custom host and scratch DB name
./pg-restore.sh \
  /backup/cmtraceopen/cmtrace-2026-04-20T020000Z.sql.gz \
  -h db.internal \
  -U cmtrace \
  -T drill_db_20260420
```

### blob-backup.sh

```bash
# rsync to a local/NAS path (default backend)
BLOB_DST=/mnt/nas/blob-backups ./blob-backup.sh

# rsync to a remote host
BLOB_DST=backup-host:/backup/cmtraceopen/blobs ./blob-backup.sh

# Azure Blob Storage (requires azcopy login or SAS token in URL)
BACKEND=azure \
BLOB_DST="https://mystorageacct.blob.core.windows.net/backups/blobs/" \
./blob-backup.sh
```

---

## Cron entries

Add the following to the crontab of the service account that runs the stack
(e.g. `crontab -e` as `cmtrace`):

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

> **Tip:** Confirm the cron ran by checking the log:
> ```bash
> tail -50 /var/log/cmtraceopen-backup.log
> tail -50 /var/log/cmtraceopen-blob-backup.log
> ```

---

## Environment variables reference

### pg-backup.sh

| Variable | Default | Description |
|---|---|---|
| `PG_HOST` | `localhost` | Postgres host |
| `PG_PORT` | `5432` | Postgres port |
| `PG_USER` | `cmtrace` | Postgres user |
| `PG_DB`   | `cmtrace` | Database to dump (single DB) |
| `BACKUP_DIR` | `/backup/cmtraceopen` | Directory to write `.sql.gz` files |
| `RETENTION_DAYS` | `30` | Delete dumps older than this many days |
| `LOG_FILE` | `/var/log/cmtraceopen-backup.log` | Log file path |

> Password: read from `~/.pgpass` (preferred). `PGPASSWORD` is honoured by libpq but should not be set inline in cron.

### pg-restore.sh

| Variable | Default | Description |
|---|---|---|
| `PG_HOST` | `localhost` | Postgres host |
| `PG_PORT` | `5432` | Postgres port |
| `PG_USER` | `postgres` | Postgres user |
| `TARGET_DB` | `cmtrace_restore_test` | Scratch DB name (never use a prod name) |

> Password: read from `~/.pgpass` (preferred).

### blob-backup.sh

| Variable | Default | Description |
|---|---|---|
| `BACKEND` | `local` | `azure` (azcopy) or `local` (rsync) |
| `BLOB_SRC` | `/var/lib/cmtraceopen/data/blobs` | Source blob directory |
| `BLOB_DST` | *(required)* | Azure URL or rsync destination |
| `LOG_FILE` | `/var/log/cmtraceopen-blob-backup.log` | Log file path |

---

## Dependencies

| Script | Tools needed |
|---|---|
| `pg-backup.sh` | `pg_dump`, `gzip` |
| `pg-restore.sh` | `psql`, `gunzip` |
| `blob-backup.sh` (local) | `rsync` |
| `blob-backup.sh` (azure) | `azcopy` |
