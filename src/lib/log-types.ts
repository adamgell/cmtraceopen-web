// TypeScript shapes mirroring serde-serialized types from
// `cmtraceopen-parser::models::log_entry`. Field names are camelCased
// because the Rust types use `#[serde(rename_all = "camelCase")]`.
//
// Optional Rust fields (`Option<T>`) are represented as `T | undefined`
// because `serde_wasm_bindgen` emits `undefined` (not `null`) for
// `Option::None`. Fields annotated `#[serde(skip_serializing_if = "…")]`
// on the Rust side may be absent on the object entirely — which JS also
// reads back as `undefined`, so a single `T | undefined` accurately covers
// both shapes.
//
// This is a minimal v0 mirror — only the fields the web viewer actually
// renders or introspects are typed strictly. Extended specialized fields
// (IIS, DNS, Panther, DHCP, CmtLog specializations, etc.) are included
// as optional so callers can widen the UI without revisiting this file.

export type Severity = "Info" | "Warning" | "Error";

export type LogFormat =
  | "Ccm"
  | "Simple"
  | "Plain"
  | "Timestamped"
  | "DnsDebug"
  | "DnsAudit"
  | "CmtLog";

export type EntryKind = "Log" | "Section" | "Iteration" | "Header";

/**
 * Span of a recognized error code inside a log message. Mirrors
 * `cmtraceopen_parser::error_db::lookup::ErrorCodeSpan` exactly.
 * `start`/`end` are UTF-16 code unit offsets (JS `String.slice` semantics).
 */
export interface ErrorCodeSpan {
  start: number;
  end: number;
  codeHex: string;
  codeDecimal: string;
  description: string;
  category: string;
}

export interface LogEntry {
  id: number;
  lineNumber: number;
  message: string;
  component: string | undefined;
  timestamp: number | undefined;
  timestampDisplay: string | undefined;
  severity: Severity;
  thread: number | undefined;
  threadDisplay: string | undefined;
  sourceFile: string | undefined;
  format: LogFormat;
  filePath: string;
  timezoneOffset: number | undefined;
  /** Absent when empty — see `#[serde(skip_serializing_if = "Vec::is_empty")]`. */
  errorCodeSpans?: ErrorCodeSpan[];
  // --- Specialized/extended fields (DHCP, Panther, IIS, DNS, CmtLog). ---
  // All of these carry `#[serde(skip_serializing_if = "Option::is_none")]`
  // on the Rust side, so they are absent (undefined) unless the relevant
  // parser produced them.
  ipAddress?: string;
  hostName?: string;
  macAddress?: string;
  resultCode?: string;
  gleCode?: string;
  setupPhase?: string;
  operationName?: string;
  httpMethod?: string;
  uriStem?: string;
  uriQuery?: string;
  statusCode?: number;
  subStatus?: number;
  timeTakenMs?: number;
  clientIp?: string;
  serverIp?: string;
  userAgent?: string;
  serverPort?: number;
  username?: string;
  win32Status?: number;
  queryName?: string;
  queryType?: string;
  responseCode?: string;
  dnsDirection?: string;
  dnsProtocol?: string;
  sourceIp?: string;
  dnsFlags?: string;
  dnsEventId?: number;
  zoneName?: string;
  entryKind?: EntryKind;
  whatif?: boolean;
  sectionName?: string;
  sectionColor?: string;
  iteration?: string;
  tags?: string[];
}

export interface ParserSelectionInfo {
  parser: string;
  implementation: string;
  provenance: string;
  parseQuality: string;
  recordFraming: string;
  dateOrder: string | undefined;
  specialization: string | undefined;
}

export interface ParseResult {
  entries: LogEntry[];
  formatDetected: LogFormat;
  parserSelection: ParserSelectionInfo;
  totalLines: number;
  parseErrors: number;
  filePath: string;
  fileSize: number;
  byteOffset: number;
}

// ---------------------------------------------------------------------------
// API-mode wire DTOs.
//
// These mirror the camelCase types in `common-wire` (Rust) that the api-server
// serializes over HTTP. Kept separate from the WASM-parser types above because
// the API surfaces a different, server-side view of entries (persisted IDs,
// file ids, millisecond timestamps) that does not need the full parser
// specialization fields. When rendering API entries through `EntryList`, the
// client maps `LogEntryDto` → `LogEntry` (see `ApiMode.tsx`).

export type DeviceStatus = "active" | "disabled" | "revoked";

export type DeviceSummary = {
  deviceId: string;
  firstSeenUtc: string;
  lastSeenUtc: string;
  hostname?: string;
  sessionCount: number;
  /** Server-reported status — absent on older api-server versions; default to "active". */
  status?: DeviceStatus;
};

export type SessionSummary = {
  sessionId: string;
  deviceId: string;
  bundleId: string;
  collectedUtc?: string;
  ingestedUtc: string;
  sizeBytes: number;
  parseState: string;
};

export type LogEntryDto = {
  entryId: number;
  fileId: string;
  lineNumber: number;
  tsMs?: number;
  severity: "Info" | "Warning" | "Error";
  component?: string;
  thread?: string;
  message: string;
  extras?: unknown;
};

export type Paginated<T> = {
  items: T[];
  nextCursor: string | null;
};
