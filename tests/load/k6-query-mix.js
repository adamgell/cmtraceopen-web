/**
 * k6 load test — operator query-mix scenario
 *
 * Simulates 10 "operator" VUs each firing a mix of read requests at
 * ~1 req/sec against the CMTrace Open api-server:
 *
 *   GET /v1/devices/{id}/sessions
 *   GET /v1/sessions/{id}
 *   GET /v1/sessions/{id}/entries
 *
 * Metrics captured:
 *   - query_duration per endpoint (p50 / p95 / p99)
 *   - query_error_rate
 *
 * Pre-requisite: the database must already contain device / session data.
 * Run k6-bundle-ingest.js first (or use the seed helper in README.md).
 *
 * Usage:
 *   BASE_URL=http://localhost:8080 \
 *   DEVICE_IDS=LOAD-DEVICE-0001,LOAD-DEVICE-0002 \
 *   k6 run tests/load/k6-query-mix.js
 *
 * Override defaults via environment variables:
 *   OPERATORS    — concurrent operator VUs                   (default: 10)
 *   DURATION     — total test duration                       (default: 5m)
 *   DEVICE_IDS   — comma-separated list of device IDs to query
 *                  (default: LOAD-DEVICE-0001 … LOAD-DEVICE-0010)
 */

import { check, sleep } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:8080";
const OPERATORS = parseInt(__ENV.OPERATORS || "10", 10);
const DURATION = __ENV.DURATION || "5m";

// Build the device-ID pool from the env var or fall back to a default set
// that matches what k6-bundle-ingest.js creates.
const DEVICE_IDS = __ENV.DEVICE_IDS
  ? __ENV.DEVICE_IDS.split(",").map((s) => s.trim()).filter(Boolean)
  : Array.from({ length: 10 }, (_, i) =>
      `LOAD-DEVICE-${(i + 1).toString().padStart(4, "0")}`,
    );

// ---------------------------------------------------------------------------
// Custom metrics
// ---------------------------------------------------------------------------

const listSessionsLatency = new Trend("query_list_sessions_duration", true);
const getSessionLatency = new Trend("query_get_session_duration", true);
const getEntriesLatency = new Trend("query_get_entries_duration", true);
const queryErrors = new Counter("query_errors_total");
const queryErrorRate = new Rate("query_error_rate");

// ---------------------------------------------------------------------------
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    operator_query_mix: {
      executor: "constant-vus",
      vus: OPERATORS,
      duration: DURATION,
    },
  },
  thresholds: {
    // p95 for all query endpoints must stay under 500 ms.
    query_list_sessions_duration: ["p(95)<500"],
    query_get_session_duration: ["p(95)<500"],
    query_get_entries_duration: ["p(95)<500"],
    query_error_rate: ["rate<0.01"],
    http_req_failed: ["rate<0.01"],
  },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Math.random() is intentionally used here. This is a load-test helper that
// picks a random element from a small array for traffic distribution — it is
// not a security-sensitive operation and does not need a CSPRNG.
// eslint-disable-next-line no-restricted-globals
function pickRandom(arr) {
  // k6 does not expose the Web Crypto API, so Math.random is the only option.
  return arr[Math.floor(Math.random() * arr.length)]; // load-test use only
}

// ---------------------------------------------------------------------------
// Default function
// ---------------------------------------------------------------------------

export default function () {
  const deviceId = pickRandom(DEVICE_IDS);
  const headers = { "Content-Type": "application/json" };

  // ---- list sessions for a device -----------------------------------------
  const listStart = Date.now();
  const listRes = http.get(
    `${BASE_URL}/v1/devices/${encodeURIComponent(deviceId)}/sessions?limit=20`,
    { headers, tags: { endpoint: "list_sessions" } },
  );
  listSessionsLatency.add(Date.now() - listStart);

  const listOk = check(listRes, {
    "list_sessions 200": (r) => r.status === 200,
  });

  if (!listOk) {
    queryErrors.add(1);
    queryErrorRate.add(1);
    sleep(1);
    return;
  }

  // ---- fetch one session + its entries ------------------------------------
  let sessions = [];
  try {
    const body = JSON.parse(listRes.body);
    // Both a plain array and a paginated envelope {items:[...]} are handled.
    sessions = Array.isArray(body) ? body : body.items || [];
  } catch (_) {
    // Body wasn't JSON — count as error and move on.
    queryErrors.add(1);
    queryErrorRate.add(1);
    sleep(1);
    return;
  }

  if (sessions.length === 0) {
    // No sessions yet — likely running before ingest has seeded data.
    queryErrorRate.add(0);
    sleep(1);
    return;
  }

  const session = pickRandom(sessions);
  const sessionId = session.session_id || session.sessionId;

  if (!sessionId) {
    queryErrors.add(1);
    queryErrorRate.add(1);
    sleep(1);
    return;
  }

  // GET /v1/sessions/{id}
  const getStart = Date.now();
  const getRes = http.get(
    `${BASE_URL}/v1/sessions/${sessionId}`,
    { headers, tags: { endpoint: "get_session" } },
  );
  getSessionLatency.add(Date.now() - getStart);

  const getOk = check(getRes, {
    "get_session 200": (r) => r.status === 200,
  });

  if (!getOk) {
    queryErrors.add(1);
    queryErrorRate.add(1);
    sleep(1);
    return;
  }

  // GET /v1/sessions/{id}/entries
  const entriesStart = Date.now();
  const entriesRes = http.get(
    `${BASE_URL}/v1/sessions/${sessionId}/entries?limit=50`,
    { headers, tags: { endpoint: "get_entries" } },
  );
  getEntriesLatency.add(Date.now() - entriesStart);

  const entriesOk = check(entriesRes, {
    // 404 is acceptable: a session may exist (finalize succeeded) but have no
    // parsed entries yet (parse worker still pending or not implemented for
    // the content kind). The load test is measuring latency, not parse state.
    "get_entries 200 or 404": (r) => r.status === 200 || r.status === 404,
  });

  if (!entriesOk) {
    queryErrors.add(1);
    queryErrorRate.add(1);
    sleep(1);
    return;
  }

  queryErrorRate.add(0);

  // ~1 req/sec per operator (three requests above + 1 s sleep ≈ 1 cycle/sec).
  sleep(1);
}
