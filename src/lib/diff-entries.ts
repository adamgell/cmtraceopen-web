// Diff classification utilities.
//
// Ported as-is from ~/repo/cmtraceopen/src/lib/diff-entries.ts. Same
// normalization rules + classifier as the desktop app, so "only-A" /
// "only-B" / "common" tagging is identical across the two codebases.
//
// When the shared @cmtrace/themes / @cmtrace/log-utils package lands,
// this file will be a one-line re-export.

import type { LogEntry } from "./log-types";

// ---------------------------------------------------------------------------
// Types (mirrored from the desktop; stable wire for DiffView).

export type EntryClassification = "common" | "only-a" | "only-b";

export interface DiffStats {
  common: number;
  onlyA: number;
  onlyB: number;
}

// ---------------------------------------------------------------------------
// Normalization
//
// Strip the noisy bits of a log line (GUIDs, timestamps, long numbers)
// before hashing into a pattern key, so two otherwise-identical entries
// with different correlation ids / request ids / timestamps collapse onto
// the same key and classify as `common`.

const GUID_RE =
  /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g;
const ISO_TS_RE =
  /\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?/g;
const COMMON_TS_RE =
  /\d{1,2}\/\d{1,2}\/\d{4}\s+\d{1,2}:\d{2}:\d{2}\s*(?:AM|PM)?/gi;
const LONG_NUM_RE = /\b\d{5,}\b/g;

export function normalizeMessage(message: string): string {
  return message
    .replace(GUID_RE, "{GUID}")
    .replace(ISO_TS_RE, "{TS}")
    .replace(COMMON_TS_RE, "{TS}")
    .replace(LONG_NUM_RE, "{NUM}")
    .toLowerCase()
    .trim();
}

/**
 * Stable pattern key per entry. Two entries share a key iff their
 * (severity, component, normalized-message) triple matches. Used as the
 * bucket for A/B intersection.
 */
export function patternKey(entry: LogEntry): string {
  const normalizedMsg = normalizeMessage(entry.message);
  const component = (entry.component ?? "").toLowerCase();
  const severity = entry.severity.toLowerCase();
  return `${severity}|${component}|${normalizedMsg}`;
}

// ---------------------------------------------------------------------------
// Classification

function buildPatternMap(entries: LogEntry[]): Map<string, number[]> {
  const map = new Map<string, number[]>();
  for (const entry of entries) {
    const key = patternKey(entry);
    const existing = map.get(key);
    if (existing) {
      existing.push(entry.id);
    } else {
      map.set(key, [entry.id]);
    }
  }
  return map;
}

export interface ClassifyResult {
  commonKeys: Set<string>;
  onlyAKeys: Set<string>;
  onlyBKeys: Set<string>;
  /** Map from `LogEntry.id` to its per-entry classification. */
  entryClassification: Map<number, EntryClassification>;
  stats: DiffStats;
}

/**
 * Classify every entry in A and B as `common` / `only-a` / `only-b`
 * based on pattern-key intersection. O(|A| + |B|); no quadratic scan.
 */
export function classifyEntries(
  entriesA: LogEntry[],
  entriesB: LogEntry[],
): ClassifyResult {
  const mapA = buildPatternMap(entriesA);
  const mapB = buildPatternMap(entriesB);

  const commonKeys = new Set<string>();
  const onlyAKeys = new Set<string>();
  const onlyBKeys = new Set<string>();

  for (const key of mapA.keys()) {
    if (mapB.has(key)) {
      commonKeys.add(key);
    } else {
      onlyAKeys.add(key);
    }
  }
  for (const key of mapB.keys()) {
    if (!mapA.has(key)) {
      onlyBKeys.add(key);
    }
  }

  const entryClassification = new Map<number, EntryClassification>();
  for (const [key, ids] of mapA) {
    const cls: EntryClassification = commonKeys.has(key) ? "common" : "only-a";
    for (const id of ids) entryClassification.set(id, cls);
  }
  for (const [key, ids] of mapB) {
    const cls: EntryClassification = commonKeys.has(key) ? "common" : "only-b";
    for (const id of ids) entryClassification.set(id, cls);
  }

  return {
    commonKeys,
    onlyAKeys,
    onlyBKeys,
    entryClassification,
    stats: {
      common: commonKeys.size,
      onlyA: onlyAKeys.size,
      onlyB: onlyBKeys.size,
    },
  };
}

export function diffFileBaseName(filePath: string): string {
  return filePath.split(/[\\/]/).pop() ?? filePath;
}
