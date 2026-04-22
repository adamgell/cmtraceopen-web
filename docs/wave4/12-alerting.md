# Wave 4 — Alerting Runbook

> **Audience:** on-call operator. This document covers the six Prometheus
> alerts shipped in `infra/observability/prometheus-rules.yaml` and what to
> do when each fires.  All alerts deliver notifications to Slack via the
> Alertmanager configuration in `infra/observability/alertmanager-config.yaml`.

---

## Table of contents

1. [Alert overview](#1-alert-overview)
2. [Alert detail](#2-alert-detail)
   - [BundleIngestFailureRateHigh](#bundleingestfailureratehigh)
   - [IngestStalled](#ingeststalled)
   - [ParseWorkerFailing](#parseworkerfailing)
   - [HealthzDown](#healthzdown)
   - [DbPoolNearExhaustion](#dbpoolnearexhaustion)
   - [DbPoolExhausted](#dbpoolexhausted)
3. [Alertmanager configuration](#3-alertmanager-configuration)
4. [Grafana dashboard](#4-grafana-dashboard)
5. [Enabling alerts](#5-enabling-alerts)

---

## 1. Alert overview

| Alert | Severity | Condition | For | Incident playbook |
|---|---|---|---|---|
| [BundleIngestFailureRateHigh](#bundleingestfailureratehigh) | 2 (High) | Failure ratio > 5 % | 10 m | [C — Bundle ingest failure rate spike](04-day2-operations.md#c-bundle-ingest-failure-rate-spike) |
| [IngestStalled](#ingeststalled) | 1 (Critical) | Zero bundles initiated OR metric absent | 30 m | [A — All devices stopped checking in](04-day2-operations.md#a-all-devices-stopped-checking-in) |
| [ParseWorkerFailing](#parseworkerfailing) | 2 (High) | Failure rate > 0.1 runs/s | 15 m | [C — Bundle ingest failure rate spike](04-day2-operations.md#c-bundle-ingest-failure-rate-spike) |
| [HealthzDown](#healthzdown) | 1 (Critical) | Scrape target unreachable | 5 m | [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping) |
| [DbPoolNearExhaustion](#dbpoolnearexhaustion) | 3 (Medium, leading) | Pool utilisation > 70 % | 5 m | [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping) |
| [DbPoolExhausted](#dbpoolexhausted) | 2 (High, lagging) | Pool utilisation > 90 % | 5 m | [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping) |

Slack channel: **#cmtrace-alerts**  
Repeat interval: Sev 1 → 1 h, Sev 2 → 4 h, Sev 3 → 12 h

---

## 2. Alert detail

### BundleIngestFailureRateHigh

**Severity:** 2 (High)

**Expression:**

```promql
(
  rate(cmtrace_ingest_bundles_finalized_total{status="failed"}[5m])
  /
  rate(cmtrace_ingest_bundles_finalized_total[5m])
) > 0.05
```

**For:** 10 minutes

**What it means:**  
More than 5 % of bundle finalize attempts are returning `status="failed"`.
This usually indicates a problem in the parse-worker, a database write error,
or a corrupt/malformed bundle from an agent.

**Immediate actions:**

1. Check the Slack notification for the current value (`$value`).
2. SSH to BigMac26 and run:
   ```bash
   docker compose logs --tail=100 api-server | grep -i "finalize\|failed"
   ```
3. Look for repeated errors on the same `device_id` — an individual device
   sending malformed bundles is the most common cause.
4. If database errors appear, check disk space:
   ```bash
   df -h /data
   ```
5. Follow incident playbook **C** for full escalation steps.

**Cross-reference:** [C — Bundle ingest failure rate spike](04-day2-operations.md#c-bundle-ingest-failure-rate-spike)

---

### IngestStalled

**Severity:** 1 (Critical)

**Expression:**

```promql
(rate(cmtrace_ingest_bundles_initiated_total[15m]) == 0)
or absent(cmtrace_ingest_bundles_initiated_total)
```

**For:** 30 minutes

**What it means:**  
No devices have initiated a new bundle upload in the last 30 minutes, OR the
metric is not being exported at all. Either all agents are offline, the
ingest endpoint (`/v1/ingest/bundles`) is not reachable, or the api-server
has not been deployed / the scrape target is misconfigured. The `absent()`
clause catches the "fresh deploy with no devices yet" + "broken scrape"
cases that the bare `rate(...) == 0` would silently miss.

**Immediate actions:**

1. Confirm the API is up: `curl -s https://<your-api-host>:8080/healthz`  
   (On the current BigMac26 host, replace `<your-api-host>` with the server's hostname or IP.)  
2. Check whether any device has sent traffic recently:
   ```bash
   docker compose logs --tail=100 api-server | grep "POST /v1/ingest"
   ```
3. If traffic stopped suddenly at a known time, check for agent deployments
   or cert renewals around that time (Intune Cloud PKI rotation is the
   most common cause of simultaneous agent silence).
4. If only one device is silent, proceed to playbook **B**.
   If all devices are silent, proceed to playbook **A**.

**Cross-reference:** [A — All devices stopped checking in](04-day2-operations.md#a-all-devices-stopped-checking-in)

---

### ParseWorkerFailing

**Severity:** 2 (High)

**Expression:**

```promql
rate(cmtrace_parse_worker_runs_total{result="failed"}[10m]) > 0.1
```

**For:** 15 minutes

**What it means:**  
The background parse worker (which converts ingested chunks into queryable
entries) is producing more than 0.1 failures per second. Bundles are being
ingested but their content is not becoming queryable.

**Immediate actions:**

1. Check worker logs:
   ```bash
   docker compose logs --tail=200 api-server | grep -i "parse\|worker\|error"
   ```
2. Look for a pattern: does the failure apply to all devices or one?
   - **All devices** — likely a schema change, database corruption, or disk
     exhaustion. Check disk space and database integrity.
   - **One device** — likely a malformed log file format. Identify the
     `device_id` from the logs, quarantine by checking
     `GET /v1/devices/{device_id}/sessions` for the failing session, and
     forward to the agent team.
3. Follow incident playbook **C** for full escalation steps.

**Cross-reference:** [C — Bundle ingest failure rate spike](04-day2-operations.md#c-bundle-ingest-failure-rate-spike)

---

### HealthzDown

**Severity:** 1 (Critical)

**Expression:**

```promql
up{job="cmtrace-api"} == 0
```

**For:** 5 minutes

**What it means:**  
Prometheus has failed to scrape `GET /metrics` from the `cmtrace-api` target
for 5 consecutive minutes. The api-server process has either crashed, become
OOM-killed, or the network path to port 8080 is broken.

**Immediate actions:**

1. SSH to BigMac26 immediately.
2. Check container status:
   ```bash
   docker compose ps
   ```
3. If the container is stopped / restarting:
   ```bash
   docker compose logs --tail=200 api-server
   ```
   Look for panic / OOM messages.
4. Restart if the cause is understood:
   ```bash
   docker compose up -d api-server
   ```
5. If the container is running but `/metrics` is unreachable, check firewall
   rules and whether the port binding is live:
   ```bash
   ss -tlnp | grep 8080
   ```
6. Follow incident playbook **D** for full escalation steps.

> **Note:** `HealthzDown` will automatically inhibit Sev-2 and Sev-3 alerts
> (via the Alertmanager inhibit rule) while the server is down, reducing
> noise during a full outage.

**Cross-reference:** [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping)

---

### DbPoolNearExhaustion

**Severity:** 3 (Medium — leading indicator)

**Expression:**

```promql
(cmtrace_db_connections_in_use / cmtrace_db_pool_max) > 0.7
```

**For:** 5 minutes

**What it means:**  
The SQLx database connection pool is over 70 % utilised — *not yet
exhausted*, but trending toward saturation. This is a deliberate leading
indicator that gives operators a chance to act (raise `pool_max`,
investigate slow queries, throttle a noisy client) before
`DbPoolExhausted` fires and request latency starts climbing.

**Immediate actions:**

1. Check the **DB Pool Utilisation** gauge on the Grafana dashboard.
2. Check whether the rise correlates with a deploy, a new device wave, or
   a known noisy query.
3. If sustained, raise `CMTRACE_DB_POOL_MAX` proactively rather than
   waiting for the Sev-2 to fire.

**Cross-reference:** [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping)

---

### DbPoolExhausted

**Severity:** 2 (High — lagging indicator)

**Expression:**

```promql
(cmtrace_db_connections_in_use / cmtrace_db_pool_max) > 0.9
```

**For:** 5 minutes

**What it means:**  
The SQLx database connection pool is over 90 % utilised. New requests that
need a database connection will queue (or time out if the pool is 100 % busy).
Left unaddressed this leads to slow query responses and eventually HTTP 500
errors on ingest and query endpoints. By the time this alert fires, the
preceding `DbPoolNearExhaustion` Sev-3 should already have flagged the
trend — if it didn't, treat that as a configuration bug.

**Immediate actions:**

1. Check current pool state via Grafana's **DB Pool Utilisation** gauge on
   the `cmtraceopen — API Observability` dashboard.
2. Identify which endpoint is driving connections:
   - High ingest load? Check `rate(cmtrace_ingest_bundles_initiated_total[5m])`.
   - High query load? Check `rate(cmtrace_http_requests_total[5m])` by path.
3. If a query storm, check for unbounded client polling (missing
   `nextCursor` handling, page size too large).
4. Increase `CMTRACE_DB_POOL_MAX` in the compose `.env` and restart:
   ```bash
   docker compose up -d api-server
   ```
   Default is 10. For the current BigMac26 hardware, values up to 25 are safe.
5. If the pool is exhausted because of a slow query (lock contention),
   look for long-running transactions in SQLite's `sqlite_master` or in the
   api-server logs.
6. Follow incident playbook **D** for general api-server degradation steps.

**Cross-reference:** [D — api-server crashlooping](04-day2-operations.md#d-api-server-crashlooping)

---

## 3. Alertmanager configuration

File: `infra/observability/alertmanager-config.yaml`

Key design decisions:

- **Single receiver** (`slack-platform`) posts to `#cmtrace-alerts`.  Email
  and PagerDuty are out of scope for v1.
- **Route by severity** — Sev-1 alerts have a 10-second `group_wait` and
  repeat every hour.  Sev-3 alerts batch for 2 minutes and repeat every
  12 hours.
- **Group by `alertname` + `severity`** — prevents multiple alerts of the
  same name from flooding the channel.
- **Inhibit rule** — `HealthzDown` suppresses Sev-2 and Sev-3 alerts on the
  same `job` label, avoiding a cascade of secondary alerts during a full
  outage.
- **Webhook URL** is stored as the placeholder `${SLACK_WEBHOOK_URL}`.
  Substitute the real URL at deploy time:

  ```bash
  # With envsubst (Linux) — validate first, then deploy:
  export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/…"
  envsubst < infra/observability/alertmanager-config.yaml \
    > /tmp/alertmanager.yaml
  amtool check-config /tmp/alertmanager.yaml
  cp /tmp/alertmanager.yaml /etc/alertmanager/alertmanager.yaml

  # Or set it as a Kubernetes Secret and reference it via
  # alertmanager.alertmanagerSpec.configSecret in kube-prometheus-stack.
  ```

---

## 4. Grafana dashboard

File: `infra/observability/grafana-dashboard.json`

Import via **Dashboards → Import → Upload JSON file** or
`POST /api/dashboards/import` with `folderId` and `overwrite: true`.

| Panel | Metric(s) | Purpose |
|---|---|---|
| HTTP Request Rate by Route | `cmtrace_http_requests_total` | Traffic distribution across routes |
| Bundle Ingest Rate by Status | `cmtrace_ingest_bundles_finalized_total` | ok / partial / failed split |
| Parse Worker Latency (p50/p90/p99) | `cmtrace_parse_worker_duration_seconds` | Worker health |
| DB Pool Utilisation | `cmtrace_db_connections_in_use` / `cmtrace_db_pool_max` | Pool saturation gauge |
| Agent Device Count | `cmtrace_device_cert_days_until_expiry` | Enrolled device count over time |
| Cert Revocations (CRL Refresh Rate) | `cmtrace_crl_refresh_total` | CRL health by result |

The dashboard uses a `DS_PROMETHEUS` template variable so it works with any
Prometheus datasource name.

---

## 5. Enabling alerts

### kube-prometheus-stack (Kubernetes)

```bash
kubectl apply -f infra/observability/prometheus-rules.yaml
```

The `PrometheusRule` CRD is picked up automatically by the Prometheus
Operator when the `prometheus: kube-prometheus` label matches the operator's
`ruleSelector`.

### Standalone Prometheus

Copy the `spec.groups` block into your `prometheus.rules.yaml` and reload:

```bash
kill -HUP $(pidof prometheus)
# or
curl -X POST http://localhost:9090/-/reload
```

Validate first:

```bash
promtool check rules infra/observability/prometheus-rules.yaml
```

### Alertmanager

```bash
# Substitute the webhook URL, validate, then deploy:
export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/…"
envsubst < infra/observability/alertmanager-config.yaml \
  > /tmp/alertmanager.yaml
amtool check-config /tmp/alertmanager.yaml
cp /tmp/alertmanager.yaml /etc/alertmanager/alertmanager.yaml
# Signal Alertmanager to reload:
curl -X POST http://localhost:9093/-/reload
```

### Prometheus scrape config

Ensure `job_name: cmtrace-api` exists in your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: cmtrace-api
    scrape_interval: 15s
    static_configs:
      - targets: ['api-server:8080']
    metrics_path: /metrics
```

The `HealthzDown` alert depends on `up{job="cmtrace-api"}` which only appears
if this exact `job_name` is used.
