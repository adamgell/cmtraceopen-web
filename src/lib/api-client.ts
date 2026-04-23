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

import { InteractionRequiredAuthError } from "@azure/msal-browser";
import { getApiScope, getMsalInstance } from "./auth-config";
import type {
  DeviceSummary,
  LogEntryDto,
  Paginated,
  SessionFile,
  SessionSummary,
} from "./log-types";

const ENV_BASE = (import.meta.env.VITE_CMTRACE_API_BASE as string | undefined) ?? "";

/**
 * Acquire an Entra access token for the api-server, or `null` if the
 * viewer is running in anonymous mode (no MSAL instance / scope configured)
 * or if no operator account is signed in yet.
 *
 * Strategy:
 *   1. acquireTokenSilent — uses the cached refresh token. Fast and
 *      non-interactive; this is the path taken on every list call once
 *      the operator has signed in once.
 *   2. On InteractionRequiredAuthError (token expired, consent revoked,
 *      conditional-access challenge), fall back to acquireTokenPopup so
 *      the operator can re-authenticate without losing their place in the
 *      viewer.
 *
 * Other errors (network, BrowserAuthError without an account, etc.) are
 * swallowed and `null` is returned — the request will go out without an
 * Authorization header and the api-server will respond 401, which the
 * existing fetch-error path renders as a banner.
 */
export async function getAccessToken(): Promise<string | null> {
  const instance = getMsalInstance();
  const scope = getApiScope();
  if (!instance || !scope) return null;

  const account =
    instance.getActiveAccount() ?? instance.getAllAccounts()[0] ?? null;
  if (!account) return null;

  try {
    const result = await instance.acquireTokenSilent({
      scopes: [scope],
      account,
    });
    return result.accessToken;
  } catch (err) {
    if (err instanceof InteractionRequiredAuthError) {
      try {
        const result = await instance.acquireTokenPopup({ scopes: [scope] });
        return result.accessToken;
      } catch {
        return null;
      }
    }
    return null;
  }
}

/**
 * Configured API base. Empty string means same-origin (dev proxy or production
 * co-located deployment). Trailing slashes are stripped so callers can write
 * `${apiBase}/v1/...` without worrying about double slashes.
 */
export const apiBase: string = ENV_BASE.replace(/\/+$/, "");

function url(path: string): string {
  return `${apiBase}${path}`;
}

async function getJson<T>(path: string, signal?: AbortSignal): Promise<T> {
  const full = url(path);
  const headers: Record<string, string> = { Accept: "application/json" };
  // Attach the Entra bearer token when MSAL is configured + signed in.
  // In anonymous mode getAccessToken() returns null and the request goes
  // out unauthenticated (works against api-server's CMTRACE_AUTH_MODE=disabled).
  const token = await getAccessToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  const res = await fetch(full, {
    method: "GET",
    headers,
    signal,
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

async function postJson<T>(path: string, body?: unknown): Promise<T> {
  const full = url(path);
  const headers: Record<string, string> = { Accept: "application/json" };
  const token = await getAccessToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
  }
  const res = await fetch(full, {
    method: "POST",
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let detail = "";
    try {
      const text = await res.text();
      if (text) detail = ` — ${text.slice(0, 200)}`;
    } catch {
      // ignore
    }
    throw new Error(`POST ${full} failed: ${res.status} ${res.statusText}${detail}`);
  }
  return (await res.json()) as T;
}

export interface ListEntriesOptions {
  /** Max entries to return. Server clamps; client default matches the v1 UI. */
  limit?: number;
  /**
   * Minimum severity floor ("Info" | "Warning" | "Error"). The server
   * treats this as a >= comparison against its numeric severity column.
   */
  severity?: "Info" | "Warning" | "Error";
  /** Inclusive lower bound on `ts_ms`. */
  afterMs?: number;
  /** Exclusive upper bound on `ts_ms`. */
  beforeMs?: number;
  /** Plain substring applied to the `message` column (server-side LIKE). */
  q?: string;
  /**
   * Restrict entries to a single file inside the session. Corresponds to
   * the `?file=<file_id>` query parameter on the api-server side (see
   * crates/api-server/src/routes/entries.rs).
   */
  file?: string;
  /** Opaque keyset cursor from a previous page's `nextCursor`. */
  cursor?: string | null;
  /** AbortSignal so the caller can cancel in-flight requests when filters change. */
  signal?: AbortSignal;
}

export interface ListFilesOptions {
  /** Max files to return. Server default 200, max 500. */
  limit?: number;
  /** Opaque keyset cursor from a previous page's `nextCursor`. */
  cursor?: string | null;
  signal?: AbortSignal;
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
  // Build a URLSearchParams so empty / undefined filters drop out cleanly
  // and values get percent-encoded correctly (q may contain spaces).
  const params = new URLSearchParams();
  params.set("limit", String(limit));
  if (opts.severity) params.set("severity", opts.severity.toLowerCase());
  if (opts.afterMs != null) params.set("after_ts", String(opts.afterMs));
  if (opts.beforeMs != null) params.set("before_ts", String(opts.beforeMs));
  if (opts.q && opts.q.trim() !== "") params.set("q", opts.q);
  if (opts.file && opts.file !== "") params.set("file", opts.file);
  if (opts.cursor) params.set("cursor", opts.cursor);
  return getJson<Paginated<LogEntryDto>>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/entries?${params.toString()}`,
    opts.signal,
  );
}

/**
 * List files ingested as part of a session. Each session typically bundles
 * many log files; this endpoint powers the Files step between Sessions and
 * Entries in the API-mode viewer.
 */
export function listFiles(
  sessionId: string,
  opts: ListFilesOptions = {},
): Promise<Paginated<SessionFile>> {
  const params = new URLSearchParams();
  if (opts.limit != null) params.set("limit", String(opts.limit));
  if (opts.cursor) params.set("cursor", opts.cursor);
  const qs = params.toString();
  const path = `/v1/sessions/${encodeURIComponent(sessionId)}/files${qs ? `?${qs}` : ""}`;
  return getJson<Paginated<SessionFile>>(path, opts.signal);
}

/** Alias matching the naming convention requested by the files panel. */
export const apiListFiles = listFiles;
/** Alias for parity with apiListFiles — some callers prefer the `api*` prefix. */
export const apiListEntries = listEntries;

/**
 * Paginated device list — supports keyset cursor from the previous page.
 * `cursor` is the `nextCursor` from the prior `Paginated<DeviceSummary>`
 * response; omit or pass `null` to start at the first page.
 */
export function listDevicesPage(
  limit = 50,
  cursor?: string | null,
): Promise<Paginated<DeviceSummary>> {
  const params = new URLSearchParams();
  params.set("limit", String(limit));
  if (cursor) params.set("cursor", cursor);
  return getJson<Paginated<DeviceSummary>>(`/v1/devices?${params.toString()}`);
}

/**
 * Disable a registered device. Requires the caller's token to carry the
 * `CmtraceOpen.Admin` app role — the api-server enforces this and returns
 * 403 for operator-only tokens.
 *
 * Returns the raw JSON response body (shape TBD by server; currently 501
 * while the MDM-side workflow is being wired up).
 */
export function disableDevice(deviceId: string): Promise<unknown> {
  return postJson<unknown>(`/v1/admin/devices/${encodeURIComponent(deviceId)}/disable`);
}
