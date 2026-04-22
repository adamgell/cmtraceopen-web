/**
 * k6 load test — bundle-ingest scenario
 *
 * Simulates 100 devices each uploading one bundle (init → chunk → finalize)
 * per minute for 5 minutes against the CMTrace Open api-server.
 *
 * Metrics captured:
 *   - bundle_finalize_duration  p50 / p95 / p99
 *   - bundle_error_rate
 *   - http_req_duration per stage (init / chunk / finalize)
 *   - parse_queue_depth  (polled from /metrics every 10 s by a dedicated VU)
 *
 * Usage:
 *   BASE_URL=http://localhost:8080 k6 run tests/load/k6-bundle-ingest.js
 *
 * Override defaults via environment variables:
 *   DEVICES      — number of concurrent virtual devices  (default: 100)
 *   DURATION     — total test duration                   (default: 5m)
 *   CHUNK_SIZE   — bytes per chunk payload               (default: 4096)
 */

import { check, sleep } from "k6";
import crypto from "k6/crypto";
import http from "k6/http";
import { Counter, Gauge, Rate, Trend } from "k6/metrics";
import { uuidv4 } from "https://jslib.k6.io/k6-utils/1.4.0/index.js";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:8080";
const DEVICES = parseInt(__ENV.DEVICES || "100", 10);
const DURATION = __ENV.DURATION || "5m";
// Each device fires one bundle per iteration; the executor paces iterations
// to ~1 per minute across all VUs.
const CHUNK_SIZE = parseInt(__ENV.CHUNK_SIZE || "4096", 10);

// ---------------------------------------------------------------------------
// Custom metrics
// ---------------------------------------------------------------------------

const finalizeLatency = new Trend("bundle_finalize_duration", true);
const initLatency = new Trend("bundle_init_duration", true);
const chunkLatency = new Trend("bundle_chunk_duration", true);
const bundleErrors = new Counter("bundle_errors_total");
const bundleErrorRate = new Rate("bundle_error_rate");
// Parse-worker queue depth scraped from /metrics by the dedicated poller VU.
const parseQueueDepth = new Gauge("parse_queue_depth");
// SQLite pool saturation — polled from /metrics by the dedicated poller VU.
const sqlitePoolSize = new Gauge("sqlite_pool_size");

// ---------------------------------------------------------------------------
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    bundle_ingest: {
      executor: "constant-vus",
      vus: DEVICES,
      duration: DURATION,
    },
    // A single extra VU polls /metrics every 10 s so queue depth + pool
    // saturation are captured in the k6 summary alongside latency numbers.
    metrics_poller: {
      executor: "constant-vus",
      vus: 1,
      duration: DURATION,
      exec: "pollMetrics",
    },
  },
  thresholds: {
    // Fail the test if p95 finalize latency exceeds 2 s or error rate > 1 %.
    bundle_finalize_duration: ["p(95)<2000"],
    bundle_error_rate: ["rate<0.01"],
    http_req_failed: ["rate<0.01"],
  },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Generate a chunk payload of the requested size (repeated ASCII).
 * Returns both the string body and its real SHA-256 hex digest so that
 * the same hash can be supplied to init's `sha256` field and finalize's
 * `finalSha256` field, matching what the server recomputes over the staged
 * bytes.
 */
function makeChunkWithHash(size) {
  const body = "A".repeat(size);
  // k6/crypto.sha256 hashes the UTF-8 encoded bytes of the string — identical
  // to what the server stages when it receives the raw octets over the wire.
  const sha = crypto.sha256(body, "hex");
  return { body, sha };
}

/**
 * Extract a named gauge value from a plain-text Prometheus /metrics scrape.
 * Returns NaN if the metric is absent.
 */
function extractGauge(metricsText, name) {
  const re = new RegExp(`^${name}\\s+(\\S+)`, "m");
  const m = metricsText.match(re);
  return m ? parseFloat(m[1]) : NaN;
}

// ---------------------------------------------------------------------------
// Metrics-poller VU — runs in parallel with device VUs
// ---------------------------------------------------------------------------

export function pollMetrics() {
  const res = http.get(`${BASE_URL}/metrics`, {
    tags: { stage: "metrics_poll" },
  });
  if (res.status === 200) {
    const depth = extractGauge(
      res.body,
      "cmtrace_parse_worker_queue_depth",
    );
    if (!isNaN(depth)) {
      parseQueueDepth.add(depth);
    }
    // Also log pool saturation for manual inspection of the k6 output.
    const poolSize = extractGauge(res.body, "cmtrace_sqlite_pool_size");
    if (!isNaN(poolSize)) {
      // Recorded as a gauge so the max across the run is visible in the summary.
      sqlitePoolSize.add(poolSize);
    }
  }
  sleep(10);
}

// ---------------------------------------------------------------------------
// setup / teardown — snapshot /metrics before and after the run
// ---------------------------------------------------------------------------

export function setup() {
  const res = http.get(`${BASE_URL}/metrics`);
  if (res.status !== 200) {
    return { metricsAvailable: false };
  }
  return { metricsAvailable: true, before: res.body };
}

export function teardown(data) {
  if (!data || !data.metricsAvailable) return;
  const res = http.get(`${BASE_URL}/metrics`);
  if (res.status === 200) {
    // Both snapshots are emitted to stdout so they can be captured in the
    // test run output and pasted into 17-capacity-results.md.
    console.log("=== /metrics BEFORE ===");
    console.log(data.before);
    console.log("=== /metrics AFTER ===");
    console.log(res.body);
  }
}

// ---------------------------------------------------------------------------
// Default function (one VU = one simulated device)
// ---------------------------------------------------------------------------

export default function () {
  // Each VU acts as a distinct device identified by its VU number.
  const deviceId = `LOAD-DEVICE-${__VU.toString().padStart(4, "0")}`;

  // Fresh bundle-id per iteration so re-runs don't collide on idempotency.
  const bundleId = uuidv4();

  const { body: chunkPayload, sha } = makeChunkWithHash(CHUNK_SIZE);

  const headers = {
    "Content-Type": "application/json",
    "x-device-id": deviceId,
  };

  // ---- init ----------------------------------------------------------------
  // Field names must match BundleInitRequest's camelCase wire shape
  // (common-wire/src/lib.rs: #[serde(rename_all = "camelCase")]).
  const initPayload = JSON.stringify({
    bundleId,
    sha256: sha,
    sizeBytes: CHUNK_SIZE,
    contentKind: "raw-file",
    deviceHint: deviceId,
  });

  const initStart = Date.now();
  const initRes = http.post(
    `${BASE_URL}/v1/ingest/bundles`,
    initPayload,
    { headers, tags: { stage: "init" } },
  );
  initLatency.add(Date.now() - initStart);

  const initOk = check(initRes, {
    "init 200": (r) => r.status === 200,
    "init has uploadId": (r) => {
      try {
        return JSON.parse(r.body).uploadId !== undefined;
      } catch (_) {
        return false;
      }
    },
  });

  if (!initOk) {
    bundleErrors.add(1);
    bundleErrorRate.add(1);
    return;
  }

  const uploadId = JSON.parse(initRes.body).uploadId;

  // ---- chunk ---------------------------------------------------------------
  const chunkStart = Date.now();
  const chunkRes = http.put(
    `${BASE_URL}/v1/ingest/bundles/${uploadId}/chunks?offset=0`,
    chunkPayload,
    {
      headers: {
        "Content-Type": "application/octet-stream",
        "x-device-id": deviceId,
      },
      tags: { stage: "chunk" },
    },
  );
  chunkLatency.add(Date.now() - chunkStart);

  const chunkOk = check(chunkRes, {
    "chunk 200": (r) => r.status === 200,
  });

  if (!chunkOk) {
    bundleErrors.add(1);
    bundleErrorRate.add(1);
    return;
  }

  // ---- finalize ------------------------------------------------------------
  // `finalSha256` is the camelCase wire name for BundleFinalizeRequest.final_sha256.
  const finalizePayload = JSON.stringify({ finalSha256: sha });

  const finalizeStart = Date.now();
  const finalizeRes = http.post(
    `${BASE_URL}/v1/ingest/bundles/${uploadId}/finalize`,
    finalizePayload,
    { headers, tags: { stage: "finalize" } },
  );
  const finalizeDuration = Date.now() - finalizeStart;
  finalizeLatency.add(finalizeDuration);

  const finalizeOk = check(finalizeRes, {
    "finalize 200": (r) => r.status === 200,
    "finalize has sessionId": (r) => {
      try {
        return JSON.parse(r.body).sessionId !== undefined;
      } catch (_) {
        return false;
      }
    },
  });

  if (!finalizeOk) {
    bundleErrors.add(1);
    bundleErrorRate.add(1);
    return;
  }

  bundleErrorRate.add(0);

  // Pace to approximately one bundle per minute per device.
  // k6's constant-vus executor runs iterations back-to-back; sleeping here
  // keeps the rate at ~1 req/min/VU without needing the ramping executor.
  // A small ±5 s jitter spreads the 100-VU wave across the minute so all
  // devices don't hit init simultaneously at t=0, t=60, t=120 … — a
  // uniform distribution better matches real fleet behaviour.
  sleep(55 + Math.random() * 10);
}
