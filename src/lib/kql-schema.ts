// Static schema for the stubbed KQL executor. Autocomplete resolves field
// names and example values from here. Mirrors the api-server's SQLite schema
// exposed through /v1/devices, /v1/sessions, /v1/files, /v1/entries.

export interface KqlField {
  name: string;
  type: "string" | "long" | "datetime";
  examples: string[];
}

export type KqlTable = "DeviceLog" | "File" | "Entry";

export const kqlSchema: Record<KqlTable, KqlField[]> = {
  DeviceLog: [
    { name: "device_id", type: "string", examples: ['"GELL-01AA310"', '"GELL-E9C0C757"'] },
    { name: "parse_state", type: "string", examples: ['"ok"', '"ok-with-fallbacks"', '"partial"', '"failed"', '"pending"'] },
    { name: "ingested_utc", type: "datetime", examples: ["ago(24h)", "ago(7d)"] },
    { name: "collected_utc", type: "datetime", examples: ["ago(24h)"] },
    { name: "size_bytes", type: "long", examples: ["1024", "1000000"] },
  ],
  File: [
    { name: "session_id", type: "string", examples: ['"019dba89..."'] },
    { name: "relative_path", type: "string", examples: ['"logs/ccmexec.log"', '"agent/agent-2026-04-24.log"'] },
    { name: "parser_kind", type: "string", examples: ['"Ccm"', '"TracingJson"', '"IisW3c"'] },
    { name: "entry_count", type: "long", examples: ["0", "1000", "1000000"] },
    { name: "parse_error_count", type: "long", examples: ["0", "94"] },
  ],
  Entry: [
    { name: "file_id", type: "string", examples: ['"019dba89..."'] },
    { name: "line_number", type: "long", examples: ["1", "42"] },
    { name: "ts_ms", type: "long", examples: ["1776872905000"] },
    { name: "severity", type: "string", examples: ['"Info"', '"Warning"', '"Error"'] },
    { name: "component", type: "string", examples: ['"Uploader"', '"DataCollection"'] },
    { name: "message", type: "string", examples: ['"retry after 5s"'] },
  ],
};

export function tablesList(): KqlTable[] {
  return ["DeviceLog", "File", "Entry"];
}

export function fieldsFor(table: string): KqlField[] {
  return kqlSchema[table as KqlTable] ?? [];
}

export const KQL_KEYWORDS = [
  "where", "summarize", "project", "extend", "join", "take", "count", "order", "by", "asc", "desc", "and", "or", "not", "in", "between",
] as const;

export const KQL_FUNCTIONS = [
  "ago", "now", "count", "countif", "sum", "avg", "min", "max", "dcount", "startofday", "endofday",
] as const;

export const KQL_OPERATORS = [
  "==", "!=", ">=", "<=", ">", "<", "has", "contains", "startswith", "endswith",
] as const;
