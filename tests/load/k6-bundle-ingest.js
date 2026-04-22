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
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";
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
 * Build a deterministic-ish fake SHA-256 hex string from a seed string.
 * We use a trivial XOR-based digest — good enough to pass the server's
 * 64-char hex validation without importing a real crypto library.
 *
 * WARNING: This is NOT cryptographically secure and must only be used for
 * load-testing purposes. It does not produce a real SHA-256 digest and
 * would be trivially collide-able in production.
 */
function fakeSha256(seed) {
  let h = new Array(32).fill(0);
  for (let i = 0; i < seed.length; i++) {
    h[i % 32] ^= seed.charCodeAt(i);
  }
  return h.map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Generate a chunk payload of the requested size (repeated ASCII).
 */
function makeChunk(size) {
  return "A".repeat(size);
}

// ---------------------------------------------------------------------------
// Default function (one VU = one simulated device)
// ---------------------------------------------------------------------------

export default function () {
  // Each VU acts as a distinct device identified by its VU number.
  const deviceId = `LOAD-DEVICE-${__VU.toString().padStart(4, "0")}`;

  // Fresh bundle-id per iteration so re-runs don't collide on idempotency.
  const bundleId = uuidv4();

  const chunkPayload = makeChunk(CHUNK_SIZE);
  const sha = fakeSha256(`${deviceId}-${bundleId}-${chunkPayload}`);

  const headers = {
    "Content-Type": "application/json",
    "x-device-id": deviceId,
  };

  // ---- init ----------------------------------------------------------------
  const initPayload = JSON.stringify({
    bundle_id: bundleId,
    sha256: sha,
    size_bytes: CHUNK_SIZE,
    content_kind: "raw-file",
    device_hint: deviceId,
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
    "init has upload_id": (r) => {
      try {
        return JSON.parse(r.body).upload_id !== undefined;
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

  const uploadId = JSON.parse(initRes.body).upload_id;

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
  const finalizePayload = JSON.stringify({ final_sha256: sha });

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
    "finalize has session_id": (r) => {
      try {
        return JSON.parse(r.body).session_id !== undefined;
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
  sleep(60);
}
