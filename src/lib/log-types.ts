// TypeScript shapes mirroring serde-serialized types from
// `cmtraceopen-parser::models::log_entry`. Field names are camelCased
// because the Rust types use `#[serde(rename_all = "camelCase")]`.
//
// This is a minimal v0 mirror — only the fields the web viewer actually
// renders or introspects are typed strictly. Unused extended fields
// (IIS, DNS, Panther, DHCP, CmtLog specializations, etc.) are elided
// here and can be added as the UI grows.

export type Severity = "Info" | "Warning" | "Error";

export type LogFormat =
  | "Ccm"
  | "Simple"
  | "Plain"
  | "Timestamped"
  | "DnsDebug"
  | "DnsAudit"
  | "CmtLog";

export interface ErrorCodeSpan {
  start: number;
  end: number;
  code: string;
  // Additional resolved metadata may be present but is not used in v0.
  [key: string]: unknown;
}

export interface LogEntry {
  id: number;
  lineNumber: number;
  message: string;
  component: string | null;
  timestamp: number | null;
  timestampDisplay: string | null;
  severity: Severity;
  thread: number | null;
  threadDisplay: string | null;
  sourceFile: string | null;
  format: LogFormat;
  filePath: string;
  timezoneOffset: number | null;
  errorCodeSpans?: ErrorCodeSpan[];
  // Optional/extended fields from specialized parsers — not surfaced in v0.
  [key: string]: unknown;
}

export interface ParserSelectionInfo {
  parser: string;
  implementation: string;
  provenance: string;
  parseQuality: string;
  recordFraming: string;
  dateOrder: string | null;
  specialization: string | null;
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
