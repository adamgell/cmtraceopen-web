// Shared DTO → LogEntry mapper.
//
// Both `ApiMode` (the list/search view) and `DeviceLogViewer` (the per-device
// overlay) fetch entries via `listEntries()` and render them through
// `EntryList`. `EntryList` expects the WASM parser's `LogEntry` shape
// (timestamp + timestampDisplay + thread-as-number), not the server wire
// `LogEntryDto` shape (tsMs + thread-as-string). Skipping this mapper is
// the bug that left the timestamp column blank for DeviceLogViewer sessions —
// the cast-via-`as unknown as` on `page.items` preserved `tsMs` but never
// populated `timestampDisplay`, so `EntryList` always fell back to the em-dash
// placeholder.
//
// Keep this module free of React imports so it stays cheap to import from
// both UI trees.

import type { LogEntry, LogEntryDto } from "./log-types";

/**
 * Map the server `LogEntryDto` to the WASM parser's `LogEntry` shape so
 * `EntryList` can render API-sourced and locally-parsed entries through the
 * same component. Fields absent on the server side (format, specialization,
 * error-code spans) are filled with conservative defaults — the list UI only
 * consumes the common columns.
 */
export function dtoToEntry(dto: LogEntryDto): LogEntry {
  const timestamp = dto.tsMs;
  const timestampDisplay =
    typeof timestamp === "number"
      ? new Date(timestamp).toISOString().replace("T", " ").replace(/\.\d+Z$/, "")
      : undefined;
  // `thread` is a string on the wire but a number on the LogEntry side;
  // preserve the original via threadDisplay so we don't lose info like
  // "tid-42" or hex ids the server may emit.
  const threadNum =
    typeof dto.thread === "string" && /^\d+$/.test(dto.thread)
      ? Number(dto.thread)
      : undefined;
  return {
    id: dto.entryId,
    lineNumber: dto.lineNumber,
    message: dto.message,
    component: dto.component,
    timestamp,
    timestampDisplay,
    severity: dto.severity,
    thread: threadNum,
    threadDisplay: dto.thread,
    sourceFile: undefined,
    format: "Plain",
    filePath: dto.fileId,
    timezoneOffset: undefined,
  };
}
