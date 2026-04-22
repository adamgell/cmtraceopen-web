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
}

function Row({ entry, top, height }: RowProps) {
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
        {entry.message}
      </div>
    </div>
  );
}
