// api-client
//
// Minimal fetch wrapper around the cmtraceopen api-server's `/v1/*` surface.
// Shares DTO shapes with `common-wire` (Rust) via `log-types.ts`.
//
// Base URL resolution:
//   - default: "" (same-origin). In dev, Vite proxies /v1 + /healthz to the
//     api-server so the viewer never sees CORS.
//   - override: set `VITE_CMTRACE_API_BASE` at build time to point at a
//     non-colocated deployment (e.g. "https://api.example.com").
//
// v1 intentionally does not paginate: the viewer grabs the first page (up to
// `limit`) and surfaces nothing about `nextCursor`. That keeps the UI small
// while pagination UX gets designed separately.
//
// All functions throw on non-2xx with a message that includes status + URL so
// error banners render something actionable. Network failures (e.g. api-server
// down) surface as the underlying TypeError from `fetch`.

import type {
  DeviceSummary,
  LogEntryDto,
  Paginated,
  SessionSummary,
} from "./log-types";

const ENV_BASE = (import.meta.env.VITE_CMTRACE_API_BASE as string | undefined) ?? "";

/**
 * Configured API base. Empty string means same-origin (dev proxy or production
 * co-located deployment). Trailing slashes are stripped so callers can write
 * `${apiBase}/v1/...` without worrying about double slashes.
 */
export const apiBase: string = ENV_BASE.replace(/\/+$/, "");

function url(path: string): string {
  return `${apiBase}${path}`;
}

async function getJson<T>(path: string): Promise<T> {
  const full = url(path);
  const res = await fetch(full, {
    method: "GET",
    headers: { Accept: "application/json" },
  });
  if (!res.ok) {
    // Include a short body snippet when present — api-server returns JSON
    // error bodies that are useful during dev.
    let detail = "";
    try {
      const text = await res.text();
      if (text) detail = ` — ${text.slice(0, 200)}`;
    } catch {
      // ignore: body drain failure shouldn't mask the status code
    }
    throw new Error(`GET ${full} failed: ${res.status} ${res.statusText}${detail}`);
  }
  return (await res.json()) as T;
}

export interface ListEntriesOptions {
  /** Max entries to return. Server clamps; client default matches the v1 UI. */
  limit?: number;
}

export function listDevices(): Promise<Paginated<DeviceSummary>> {
  return getJson<Paginated<DeviceSummary>>("/v1/devices");
}

export function listSessions(deviceId: string): Promise<Paginated<SessionSummary>> {
  // Encode because device ids from the agent hint are free-form strings.
  return getJson<Paginated<SessionSummary>>(
    `/v1/devices/${encodeURIComponent(deviceId)}/sessions`,
  );
}

export function listEntries(
  sessionId: string,
  opts: ListEntriesOptions = {},
): Promise<Paginated<LogEntryDto>> {
  const limit = opts.limit ?? 500;
  return getJson<Paginated<LogEntryDto>>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/entries?limit=${limit}`,
  );
}
