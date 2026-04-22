# Wave 4 — DR Rehearsal Runbook

> **Audience:** the on-call operator (today: Adam).  
> This runbook governs the *quarterly* disaster-recovery rehearsal programme.
> Each drill is single-operator, time-boxed to two hours, and produces an
> entry in [`dr-rehearsal-history.md`](dr-rehearsal-history.md).

---

## Table of contents

1. [Cadence](#1-cadence)
2. [Annual scenario rotation](#2-annual-scenario-rotation)
3. [Drill template](#3-drill-template)
   - [Q1 — Full server loss](#q1--full-server-loss)
   - [Q2 — Blob storage corruption](#q2--blob-storage-corruption)
   - [Q3 — Cloud PKI tenant outage](#q3--cloud-pki-tenant-outage)
   - [Q4 — Postgres logical corruption](#q4--postgres-logical-corruption)
4. [Post-mortem template](#4-post-mortem-template)
5. [Recording results](#5-recording-results)

---

## 1. Cadence

| Quarter | Calendar window         | Trigger date (2nd Tuesday) |
|---------|-------------------------|----------------------------|
| Q1      | January                 | 2026-01-13 ← **first scheduled drill** |
| Q2      | April                   | 2026-04-14 |
| Q3      | July                    | 2026-07-14 |
| Q4      | October                 | 2026-10-13 |

Repeat on the **second Tuesday** of the opening month of each quarter.
If that date falls within a scheduled freeze window, slip to the following
Tuesday and note the reason in `dr-rehearsal-history.md`.

> **First scheduled drill:** 2026-01-13 — Q1 scenario (full server loss).

---

## 2. Annual scenario rotation

One scenario per quarter, rotating predictably so every failure mode is
exercised at least once per year:

| Quarter | Scenario | Primary concern |
|---------|----------|-----------------|
| Q1 | Full server loss | Restore from latest Postgres dump + blob rsync |
| Q2 | Blob storage corruption | Re-fetch from agent-side local queues (last 30 d) |
| Q3 | Cloud PKI tenant outage | Agent-side queueing, no data loss during fallback |
| Q4 | Postgres logical corruption | Roll back to N-1 backup, document RTO/RPO loss |

---

## 3. Drill template

Every drill follows the same skeleton:

```
Pre-conditions  →  Action sequence  →  Success criteria  →  Post-mortem
```

Each section below is a **self-contained step-by-step drill**. Copy the
relevant section into a scratch doc at the start of the drill window.

---

### Q1 — Full server loss

**Scenario:** BigMac26 VM is unrecoverable (disk failure, accidental deletion,
or provider outage). Restore the full stack from off-host backups.

**Time box:** 2 h  
**RTO target:** 4 h (drill passes if key milestones are reached in ≤ 2 h)  
**RPO target:** 24 h (last nightly Postgres dump)

#### Pre-conditions (verify before starting the clock)

- [ ] Off-host backup set from the previous night exists and is accessible:
  ```bash
  ls -lh /backup/cmtraceopen/$(date -d yesterday +%F)/
  # expect: cmtraceopen.sql  blobs/
  ```
- [ ] A replacement host or second VM is available and reachable via SSH.
- [ ] Docker + Docker Compose installed on the replacement host.
- [ ] `docker-compose.yml` and `.env` are checked into source control (no
  secrets-only-on-host).

#### Action sequence

1. **[T+0]** Note start time in your scratch doc.

2. **[T+0–5 min]** Identify the most recent backup date:
   ```bash
   BACKUP_DATE=$(ls /backup/cmtraceopen/ | sort | tail -1)
   echo "Restoring from: $BACKUP_DATE"
   ```

3. **[T+5–20 min]** Provision the replacement host (or verify the spare VM
   is clean). Record the new hostname as `NEWHOST`.

4. **[T+20–35 min]** Restore Postgres:
   ```bash
   # On the replacement host
   docker run --rm -d \
     --name pg-restore \
     -e POSTGRES_PASSWORD=changeme \
     -v /backup/cmtraceopen/${BACKUP_DATE}/:/backup \
     postgres:16-alpine

   # Wait ~10 s for Postgres to start, then:
   docker exec -i pg-restore \
     psql -U postgres -c "CREATE DATABASE cmtraceopen;"

   docker exec -i pg-restore \
     psql -U postgres -d cmtraceopen \
     < /backup/cmtraceopen/${BACKUP_DATE}/cmtraceopen.sql

   docker stop pg-restore
   ```

5. **[T+35–50 min]** Restore blob store:
   ```bash
   rsync -avz --delete \
     /backup/cmtraceopen/${BACKUP_DATE}/blobs/ \
     ${NEWHOST}:/var/lib/cmtraceopen/data/blobs/
   ```

6. **[T+50–65 min]** Bring up the stack on the replacement host:
   ```bash
   ssh ${NEWHOST} 'cd /srv/cmtraceopen && docker compose up -d'
   ```

7. **[T+65–75 min]** Verify health endpoints respond:
   ```bash
   curl -k https://${NEWHOST}/healthz   # expect 200
   curl -k https://${NEWHOST}/readyz    # expect 200
   ```

8. **[T+75–85 min]** Spot-check data integrity:
   ```bash
   # Session count should be > 0 and match pre-drill snapshot
   curl -sk https://${NEWHOST}/v1/sessions | jq '.total'
   ```

9. **[T+85–90 min]** Verify agent re-queued bundles start arriving (tail
   the api-server log for ingest events for ~5 min).

10. **[T+90]** Note end time. Tear down the test host (do **not** promote to
    production without a separate change-management approval).

#### Success criteria

- [ ] `GET /healthz` returns HTTP 200 on replacement host.
- [ ] `GET /readyz` returns HTTP 200 on replacement host.
- [ ] Session count after restore matches the pre-drill snapshot ± 0 rows.
- [ ] At least one agent-submitted bundle is visible in the logs after
  restore.
- [ ] Total elapsed time ≤ 2 h.

#### Notes / known caveats

- The agent's resumable uploader automatically re-inits partial uploads;
  no manual intervention is needed per-device.
- In-flight bundles queued client-side retry on the next agent tick
  (RetryPolicy: 3 × at 1 s / 5 s / 30 s, then exponential backoff via
  `mark_failed`).

---

### Q2 — Blob storage corruption

**Scenario:** The blob store on BigMac26 has been silently corrupted (bit-rot
or a bad `rsync` run). Re-fetch missing/corrupt blobs from agent local queues,
which hold the last 30 days of data.

**Time box:** 2 h  
**RTO target:** 8 h (drill proves the mechanism; full fleet re-delivery may
take longer)  
**RPO target:** 30 d (agent-side queue retention window)

#### Pre-conditions

- [ ] You can enumerate the corrupted blob keys (query Postgres for rows whose
  blob SHA does not match the on-disk file):
  ```bash
  # Pseudo-query — adapt to actual schema
  psql -U cmtraceopen -d cmtraceopen -c \
    "SELECT session_id, bundle_sha256 FROM bundles WHERE status = 'stored';" \
    | head -20
  ```
- [ ] At least one test device's agent queue is reachable and contains recent
  bundles (`%ProgramData%\cmtraceopen-agent\queue\` on Windows).

#### Action sequence

1. **[T+0]** Note start time.

2. **[T+0–15 min]** Identify the set of corrupt/missing blobs:
   ```bash
   # List blobs recorded in Postgres
   psql -U cmtraceopen -d cmtraceopen -t -A -c \
     "SELECT blob_path FROM bundles WHERE status = 'stored';" \
     > /tmp/db-blobs.txt

   # List blobs actually on disk
   find /var/lib/cmtraceopen/data/blobs/ -type f \
     > /tmp/disk-blobs.txt

   # Diff — lines in db-blobs.txt not in disk-blobs.txt are missing
   comm -23 <(sort /tmp/db-blobs.txt) <(sort /tmp/disk-blobs.txt) \
     > /tmp/missing-blobs.txt
   wc -l /tmp/missing-blobs.txt
   ```

3. **[T+15–30 min]** For each missing blob, mark the corresponding bundle
   rows as `pending_redelivery` so the api-server will re-accept them:
   ```bash
   # Adapt session_ids from the missing-blobs list
   psql -U cmtraceopen -d cmtraceopen -c \
     "UPDATE bundles SET status = 'pending_redelivery'
      WHERE blob_path IN (SELECT * FROM /tmp/missing-blobs.txt);"
   ```

4. **[T+30–45 min]** Trigger re-upload from a test agent by cycling the
   cmtraceopen-agent service:
   ```powershell
   # On a test Windows device
   Restart-Service cmtraceopen-agent
   ```
   Tail the api-server log and confirm the bundle arrives:
   ```bash
   docker compose logs -f api-server | grep "bundle_ingest"
   ```

5. **[T+45–90 min]** For a fleet-wide corruption event, the same
   `pending_redelivery` flag triggers agents on their next tick (default
   interval: 15 min). Monitor ingest rate until the missing blob count
   reaches zero or the drill window closes.

6. **[T+90]** Note end time.

#### Success criteria

- [ ] At least one previously-missing blob has been re-delivered and
  `status` column returns to `stored`.
- [ ] No duplicate ingest errors in the api-server log (idempotency check).
- [ ] `GET /healthz` and `GET /readyz` remain green throughout.
- [ ] Total elapsed time ≤ 2 h.

---

### Q3 — Cloud PKI tenant outage

**Scenario:** The Microsoft Cloud PKI tenant is unreachable. New device-cert
issuance is blocked. Prove that agent-side queueing absorbs in-flight bundles
with no data loss, and that the dual-CA fallback lets already-issued devices
continue ingesting.

**Time box:** 2 h  
**RTO target:** 24 h (re-issue certs after PKI restored; data loss = 0)  
**RPO target:** 0 (no bundles lost; queued client-side until certs renewed)

#### Pre-conditions

- [ ] You have a test device with a **valid, unexpired** cert already issued
  (simulates devices in the field during the outage).
- [ ] You have a second test device with **no cert yet** (simulates a newly
  enrolled device that would be blocked).
- [ ] Backup of `CMTRACE_CLIENT_CA_BUNDLE` value is noted:
  ```bash
  docker compose exec api-server \
    printenv CMTRACE_CLIENT_CA_BUNDLE | md5sum
  ```

#### Action sequence

1. **[T+0]** Note start time.

2. **[T+0–10 min]** Simulate PKI outage: comment out the live Cloud PKI
   issuing CA from `CMTRACE_CLIENT_CA_BUNDLE` in `.env`, restart the
   api-server:
   ```bash
   # In .env, change CMTRACE_CLIENT_CA_BUNDLE to a stub / expired cert
   docker compose up -d api-server
   ```

3. **[T+10–20 min]** Confirm the second test device (no cert) cannot ingest:
   ```bash
   # On the uncertified device — expect TLS handshake failure
   curl -v https://bigmac26/v1/bundles/start 2>&1 | grep -i "alert\|error"
   ```

4. **[T+20–35 min]** Confirm the first test device (valid cert) **can**
   still ingest using the retained old CA in the bundle:
   ```bash
   docker compose logs -f api-server | grep "bundle_ingest"
   ```
   If the old CA was also removed, bundles queue client-side. Verify the
   Windows event log on the test device shows `QueuedForRetry` entries, not
   dropped bundles.

5. **[T+35–50 min]** Restore the full CA bundle (simulate PKI coming back):
   ```bash
   # Restore CMTRACE_CLIENT_CA_BUNDLE to original value
   docker compose up -d api-server
   ```

6. **[T+50–70 min]** Restart the cmtraceopen-agent on both test devices and
   verify queued bundles drain:
   ```powershell
   Restart-Service cmtraceopen-agent
   ```
   Watch api-server logs for back-fill ingest events.

7. **[T+70–80 min]** Count bundles in Postgres — should equal the total
   generated during the outage window (zero loss).

8. **[T+80–90 min]** Document the dual-CA window boundaries in the
   history entry (see §5).

9. **[T+90]** Note end time. Restore `.env` to production values.

#### Success criteria

- [ ] Devices with valid certs continued to ingest during simulated outage.
- [ ] Devices without certs queued bundles client-side (no drops logged).
- [ ] After CA bundle restored, all queued bundles delivered successfully.
- [ ] Bundle count in Postgres after drill = bundle count before drill + all
  bundles generated during drill window (zero data loss).
- [ ] Total elapsed time ≤ 2 h.

---

### Q4 — Postgres logical corruption

**Scenario:** A bad migration or operator error has introduced logical
corruption into the Postgres database (e.g., orphaned FK rows, wrong
column values). Roll back to the N-1 backup, accept the 1-day RPO, and
document actual RTO and RPO.

**Time box:** 2 h  
**RTO target:** 4 h (drill verifies the mechanism)  
**RPO target:** 24 h (accept losing up to 1 day of metadata; bundles are
safe in blob store and can be re-associated)

#### Pre-conditions

- [ ] Two consecutive nightly Postgres dumps exist:
  ```bash
  ls -lt /backup/cmtraceopen/ | head -3
  # expect two dated directories
  ```
- [ ] Blob store is intact (this scenario is DB-only corruption).
- [ ] The N-1 dump path is identified:
  ```bash
  N1_DATE=$(ls /backup/cmtraceopen/ | sort | tail -2 | head -1)
  echo "N-1 backup: $N1_DATE"
  ```

#### Action sequence

1. **[T+0]** Note start time. Record current row counts as baseline:
   ```bash
   psql -U cmtraceopen -d cmtraceopen -c \
     "SELECT relname, n_live_tup FROM pg_stat_user_tables ORDER BY relname;"
   ```

2. **[T+0–5 min]** Stop the api-server to prevent new writes:
   ```bash
   docker compose stop api-server
   ```

3. **[T+5–10 min]** Dump the corrupted DB for forensics (do **not** skip
   this — needed for post-mortem):
   ```bash
   pg_dump -h localhost -U cmtraceopen cmtraceopen \
     > /tmp/cmtraceopen-corrupt-$(date +%F-%H%M).sql
   ```

4. **[T+10–30 min]** Drop and recreate the database, then restore from N-1:
   ```bash
   psql -U postgres -c "DROP DATABASE cmtraceopen;"
   psql -U postgres -c "CREATE DATABASE cmtraceopen;"
   psql -U postgres -d cmtraceopen \
     < /backup/cmtraceopen/${N1_DATE}/cmtraceopen.sql
   ```

5. **[T+30–40 min]** Restart the api-server:
   ```bash
   docker compose up -d api-server
   curl -k https://localhost/healthz   # expect 200
   curl -k https://localhost/readyz    # expect 200
   ```

6. **[T+40–55 min]** Verify row counts — should be N-1 snapshot values
   (lower than baseline by ~1 day of ingest):
   ```bash
   psql -U cmtraceopen -d cmtraceopen -c \
     "SELECT relname, n_live_tup FROM pg_stat_user_tables ORDER BY relname;"
   ```

7. **[T+55–70 min]** Cross-reference blob store: blobs for the lost 24-h
   window still exist on disk. Identify them:
   ```bash
   # Blobs on disk with no matching DB row = recoverable
   find /var/lib/cmtraceopen/data/blobs/ -newer \
     /backup/cmtraceopen/${N1_DATE}/cmtraceopen.sql -type f \
     > /tmp/orphaned-blobs.txt
   wc -l /tmp/orphaned-blobs.txt
   ```
   Record the orphan count in the history entry as the actual data at risk.

8. **[T+70–80 min]** Document actual RTO and RPO:
   - **Actual RTO** = T+55 (api-server healthy) − T+0 (start)
   - **Actual RPO** = timestamp of newest row in N-1 dump vs. now

9. **[T+80–90 min]** Decide (out of scope for drill): schedule a follow-up
   task to write a re-association script for the orphaned blobs or accept
   the loss.

10. **[T+90]** Note end time. Restore to production DB if drill was on a
    non-production host, or document the data-loss window in the incident log.

#### Success criteria

- [ ] N-1 backup restores cleanly with no Postgres errors.
- [ ] `GET /healthz` and `GET /readyz` return 200 after restore.
- [ ] Orphaned-blob count recorded (blobs at risk, not yet lost).
- [ ] Actual RTO and RPO values documented in history entry.
- [ ] Total elapsed time ≤ 2 h.

---

## 4. Post-mortem template

After each drill, complete this template and append it to
[`dr-rehearsal-history.md`](dr-rehearsal-history.md).

```markdown
## Drill: <Quarter> <Year> — <Scenario name>

- **Date:** YYYY-MM-DD
- **Operator:** <name>
- **Scenario:** Q<N> — <one-line description>
- **Start time (UTC):** HH:MM
- **End time (UTC):** HH:MM
- **Total elapsed:** Xh Ym
- **Outcome:** PASS | PARTIAL | FAIL

### Pre-conditions met?

- [ ] <pre-condition 1>
- [ ] <pre-condition 2>

### Action sequence notes

Step-by-step notes. Highlight any deviations from the runbook.

| Step | Expected | Actual | OK? |
|------|----------|--------|-----|
| 1 | ... | ... | ✓ / ✗ |

### Success criteria results

- [ ] Criterion 1 — result
- [ ] Criterion 2 — result

### What went well

- Bullet 1
- Bullet 2

### What didn't go well

- Bullet 1
- Bullet 2

### Runbook fixes needed before next quarter

- [ ] Fix 1 (owner: <name>, due: YYYY-MM-DD)
- [ ] Fix 2

### Actual RTO / RPO (if applicable)

- **RTO:** Xh Ym
- **RPO:** Xh (data window lost or at risk)
```

---

## 5. Recording results

Append each completed drill entry to
[`dr-rehearsal-history.md`](dr-rehearsal-history.md) **within 48 hours** of
the drill date. The history file is **append-only** — never edit or delete
past entries.

If a drill is skipped or deferred, add a one-line note:

```markdown
## Skipped: Q<N> <Year> — <reason> (rescheduled to YYYY-MM-DD)
```

---

*Cross-reference: [`04-day2-operations.md` §5 Disaster recovery](04-day2-operations.md#5-disaster-recovery)*
