import { useMemo, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { LogEntry, Severity } from "../lib/log-types";
import { applyFilters, type Filters } from "./FilterBar";

export interface EntryListProps {
  entries: LogEntry[];
  /**
   * Optional client-side filter. When omitted, the list renders `entries`
   * as-is (preserving the pre-filter behaviour for callers that haven't
   * adopted FilterBar yet, e.g. callers that push filtering fully
   * server-side).
   */
  filters?: Filters;
}

/**
 * Match span used by the row renderer to wrap the search needle in a
 * `<mark>` element. UTF-16 code-unit offsets, half-open `[start, end)` —
 * matches `String.slice` semantics. Empty list = render as plain text.
 */
interface HighlightSpan {
  start: number;
  end: number;
}

const ROW_HEIGHT = 24;

const SEVERITY_COLORS: Record<Severity, { bg: string; fg: string; label: string }> = {
  Info: { bg: "transparent", fg: "#555", label: "INFO" },
  Warning: { bg: "#fff7e6", fg: "#b45309", label: "WARN" },
  Error: { bg: "#fef2f2", fg: "#b91c1c", label: "ERR" },
};

/**
 * Virtualized single-line-per-entry table. Columns: line number, timestamp,
 * severity, component, thread, message. Message is truncated with ellipsis;
 * the native `title` attribute surfaces the full text on hover.
 */
export function EntryList({ entries, filters }: EntryListProps) {
  const parentRef = useRef<HTMLDivElement>(null);

  // Derive the filtered view. Empty filters short-circuit to the input
  // reference inside applyFilters so `displayEntries === entries` — no
  // virtualizer thrash when the user hasn't narrowed.
  const displayEntries = useMemo(
    () => (filters ? applyFilters(entries, filters) : entries),
    [entries, filters],
  );

  // Lowercased search needle used by the per-row highlighter. We compute
  // it once here (rather than per-row) so 50k rendered rows don't each
  // pay for a `.toLowerCase()`. Empty needle disables highlighting.
  const searchNeedle = useMemo(
    () => filters?.search.trim().toLowerCase() ?? "",
    [filters?.search],
  );

  const virtualizer = useVirtualizer({
    count: displayEntries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  });

  const items = virtualizer.getVirtualItems();

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
        border: "1px solid #ddd",
        borderRadius: 4,
        overflow: "hidden",
        fontFamily:
          "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
        fontSize: 12,
      }}
    >
      <HeaderRow />
      <div
        ref={parentRef}
        style={{ flex: 1, overflow: "auto", contain: "strict" }}
      >
        <div
          style={{
            height: virtualizer.getTotalSize(),
            width: "100%",
            position: "relative",
          }}
        >
          {items.map((vi) => {
            const entry = displayEntries[vi.index];
            if (!entry) return null;
            return (
              <Row
                key={vi.key}
                entry={entry}
                top={vi.start}
                height={ROW_HEIGHT}
                searchNeedle={searchNeedle}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

const COLUMNS = {
  line: "80px",
  timestamp: "200px",
  severity: "60px",
  component: "180px",
  thread: "70px",
  message: "1fr",
} as const;

const GRID_TEMPLATE = `${COLUMNS.line} ${COLUMNS.timestamp} ${COLUMNS.severity} ${COLUMNS.component} ${COLUMNS.thread} ${COLUMNS.message}`;

function HeaderRow() {
  const headerCell: React.CSSProperties = {
    padding: "6px 8px",
    fontWeight: 600,
    color: "#444",
    borderRight: "1px solid #e5e5e5",
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  };
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: GRID_TEMPLATE,
        background: "#f5f5f5",
        borderBottom: "1px solid #ddd",
        fontSize: 11,
        textTransform: "uppercase",
        letterSpacing: "0.04em",
      }}
    >
      <div style={headerCell}>Line</div>
      <div style={headerCell}>Timestamp</div>
      <div style={headerCell}>Sev</div>
      <div style={headerCell}>Component</div>
      <div style={headerCell}>Thread</div>
      <div style={{ ...headerCell, borderRight: "none" }}>Message</div>
    </div>
  );
}

interface RowProps {
  entry: LogEntry;
  top: number;
  height: number;
  /**
   * Pre-lowercased search needle. Empty string means "don't highlight" —
   * the row renders as a plain text node, no `<mark>` wrapping.
   */
  searchNeedle: string;
}

function Row({ entry, top, height, searchNeedle }: RowProps) {
  const sev = SEVERITY_COLORS[entry.severity];
  const cell: React.CSSProperties = {
    padding: "0 8px",
    lineHeight: `${height}px`,
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
    borderRight: "1px solid #f0f0f0",
  };
  return (
    <div
      style={{
        position: "absolute",
        top,
        left: 0,
        width: "100%",
        height,
        display: "grid",
        gridTemplateColumns: GRID_TEMPLATE,
        background: sev.bg,
        borderBottom: "1px solid #f5f5f5",
      }}
    >
      <div style={{ ...cell, color: "#999", textAlign: "right" }}>
        {entry.lineNumber}
      </div>
      <div style={{ ...cell, color: "#666" }}>
        {entry.timestampDisplay ?? "—"}
      </div>
      <div
        style={{
          ...cell,
          color: sev.fg,
          fontWeight: 600,
        }}
      >
        {sev.label}
      </div>
      <div style={cell} title={entry.component ?? undefined}>
        {entry.component ?? ""}
      </div>
      <div style={{ ...cell, color: "#666", textAlign: "right" }}>
        {entry.threadDisplay ?? (entry.thread != null ? String(entry.thread) : "")}
      </div>
      <div
        style={{ ...cell, borderRight: "none", color: "#222" }}
        title={entry.message}
      >
        {searchNeedle
          ? renderHighlighted(entry.message, searchNeedle)
          : entry.message}
      </div>
    </div>
  );
}

/**
 * Render `text` with each occurrence of `needle` (case-insensitive)
 * wrapped in a `<mark>`. Returns the bare string when there's no match
 * so the common case stays a single text node — important for the row
 * virtualizer, which mounts/unmounts hundreds of rows on scroll.
 *
 * Allocation strategy: build the match-span list first, then walk it
 * in one pass. We avoid `String.matchAll` because the needle isn't a
 * regex and escaping it just to call a regex method is wasteful.
 */
function renderHighlighted(text: string, needle: string): React.ReactNode {
  const spans = collectMatches(text, needle);
  if (spans.length === 0) return text;

  const out: React.ReactNode[] = [];
  let cursor = 0;
  for (let i = 0; i < spans.length; i++) {
    const { start, end } = spans[i]!;
    if (start > cursor) {
      out.push(text.slice(cursor, start));
    }
    out.push(
      <mark
        key={i}
        style={{
          background: "#fde68a",
          color: "inherit",
          padding: 0,
          borderRadius: 2,
        }}
      >
        {text.slice(start, end)}
      </mark>,
    );
    cursor = end;
  }
  if (cursor < text.length) out.push(text.slice(cursor));
  return out;
}

function collectMatches(text: string, lowerNeedle: string): HighlightSpan[] {
  if (!lowerNeedle) return [];
  const haystack = text.toLowerCase();
  const out: HighlightSpan[] = [];
  let from = 0;
  while (from <= haystack.length) {
    const idx = haystack.indexOf(lowerNeedle, from);
    if (idx === -1) break;
    out.push({ start: idx, end: idx + lowerNeedle.length });
    from = idx + lowerNeedle.length;
  }
  return out;
}
