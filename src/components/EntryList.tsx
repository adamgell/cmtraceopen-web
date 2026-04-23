import { useMemo, useRef, useState, useEffect, useCallback } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { tokens } from "@fluentui/react-components";
import type { LogEntry, Severity } from "../lib/log-types";
import { applyFilters, type Filters } from "./FilterBar";
import { useTheme } from "../lib/theme-context";
import {
  LOG_MONOSPACE_FONT_FAMILY,
  LOG_UI_FONT_FAMILY,
} from "../lib/log-accessibility";

export interface EntryListProps {
  entries: LogEntry[];
  /** Optional client-side filter. When omitted, renders `entries` as-is. */
  filters?: Filters;
}

/** [start, end) offsets of search-highlight spans within a message. */
interface HighlightSpan {
  start: number;
  end: number;
}

const ROW_HEIGHT = 22;
const SEVERITY_COL_WIDTH = 28;

/**
 * CMTrace-style log grid. Columns: severity dot, line #, timestamp,
 * component, thread, message. Row background uses the active theme's
 * severity palette; the selected row is outlined and drives a bottom
 * detail pane showing the full entry.
 *
 * The palette comes from `useTheme().theme.severityPalette`, which
 * matches the desktop app's `src/lib/themes/palettes.ts` exactly, so
 * switching themes reflows both the grid and the detail pane to match
 * the desktop look.
 */
export function EntryList({ entries, filters }: EntryListProps) {
  const { theme } = useTheme();
  const palette = theme.severityPalette;
  const parentRef = useRef<HTMLDivElement>(null);

  const displayEntries = useMemo(
    () => (filters ? applyFilters(entries, filters) : entries),
    [entries, filters],
  );

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

  // Selection: 0-based index into displayEntries. null = nothing selected.
  const [selectedIdx, setSelectedIdx] = useState<number | null>(null);

  // Clamp selection when the filter changes under us so we don't hold
  // a dangling index.
  useEffect(() => {
    if (selectedIdx != null && selectedIdx >= displayEntries.length) {
      setSelectedIdx(null);
    }
  }, [displayEntries.length, selectedIdx]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (displayEntries.length === 0) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIdx((prev) => {
          const next = prev == null ? 0 : Math.min(prev + 1, displayEntries.length - 1);
          virtualizer.scrollToIndex(next, { align: "auto" });
          return next;
        });
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIdx((prev) => {
          const next = prev == null ? 0 : Math.max(prev - 1, 0);
          virtualizer.scrollToIndex(next, { align: "auto" });
          return next;
        });
      } else if (e.key === "Home") {
        e.preventDefault();
        setSelectedIdx(0);
        virtualizer.scrollToIndex(0, { align: "start" });
      } else if (e.key === "End") {
        e.preventDefault();
        const last = displayEntries.length - 1;
        setSelectedIdx(last);
        virtualizer.scrollToIndex(last, { align: "end" });
      }
    },
    [displayEntries.length, virtualizer],
  );

  const selectedEntry =
    selectedIdx != null ? displayEntries[selectedIdx] ?? null : null;

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
        border: `1px solid ${tokens.colorNeutralStroke1}`,
        borderRadius: tokens.borderRadiusMedium,
        overflow: "hidden",
        fontFamily: LOG_MONOSPACE_FONT_FAMILY,
        fontSize: tokens.fontSizeBase200,
        background: tokens.colorNeutralBackground1,
        color: tokens.colorNeutralForeground1,
      }}
    >
      <HeaderRow />
      <div
        ref={parentRef}
        tabIndex={0}
        role="listbox"
        aria-label="Log entries"
        onKeyDown={onKeyDown}
        style={{
          flex: 1,
          overflow: "auto",
          contain: "strict",
          outline: "none",
        }}
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
                index={vi.index}
                top={vi.start}
                height={ROW_HEIGHT}
                searchNeedle={searchNeedle}
                isSelected={vi.index === selectedIdx}
                onClick={setSelectedIdx}
                palette={palette}
              />
            );
          })}
        </div>
      </div>
      <DetailPane entry={selectedEntry} palette={palette} />
    </div>
  );
}

const COLUMNS = {
  severity: `${SEVERITY_COL_WIDTH}px`,
  line: "64px",
  timestamp: "180px",
  component: "180px",
  thread: "70px",
  message: "1fr",
} as const;

const GRID_TEMPLATE = `${COLUMNS.severity} ${COLUMNS.line} ${COLUMNS.timestamp} ${COLUMNS.component} ${COLUMNS.thread} ${COLUMNS.message}`;

function HeaderRow() {
  const headerCell: React.CSSProperties = {
    padding: "6px 8px",
    fontWeight: 600,
    color: tokens.colorNeutralForeground2,
    borderRight: `1px solid ${tokens.colorNeutralStroke2}`,
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  };
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: GRID_TEMPLATE,
        background: tokens.colorNeutralBackground2,
        borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: tokens.fontSizeBase100,
        textTransform: "uppercase",
        letterSpacing: "0.04em",
        position: "sticky",
        top: 0,
        zIndex: 1,
      }}
    >
      <div style={{ ...headerCell, textAlign: "center" }}>•</div>
      <div style={headerCell}>Line</div>
      <div style={headerCell}>Timestamp</div>
      <div style={headerCell}>Component</div>
      <div style={headerCell}>Thread</div>
      <div style={{ ...headerCell, borderRight: "none" }}>Message</div>
    </div>
  );
}

interface RowProps {
  entry: LogEntry;
  index: number;
  top: number;
  height: number;
  searchNeedle: string;
  isSelected: boolean;
  onClick: (idx: number) => void;
  palette: ReturnType<typeof useTheme>["theme"]["severityPalette"];
}

function Row({
  entry,
  index,
  top,
  height,
  searchNeedle,
  isSelected,
  onClick,
  palette,
}: RowProps) {
  const severityColors = paletteForSeverity(entry.severity, palette);

  const cell: React.CSSProperties = {
    padding: "0 8px",
    lineHeight: `${height}px`,
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  };

  return (
    <div
      role="option"
      aria-selected={isSelected}
      onClick={() => onClick(index)}
      style={{
        position: "absolute",
        top,
        left: 0,
        width: "100%",
        height,
        display: "grid",
        gridTemplateColumns: GRID_TEMPLATE,
        background: severityColors.background,
        color: severityColors.text,
        cursor: "pointer",
        boxShadow: isSelected
          ? `inset 3px 0 0 ${tokens.colorBrandBackground}, inset 0 0 0 1px ${tokens.colorBrandStroke1}`
          : "inset 3px 0 0 transparent",
      }}
    >
      <div
        style={{
          ...cell,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: 0,
        }}
        aria-label={entry.severity}
      >
        <SeverityDot severity={entry.severity} palette={palette} />
      </div>
      <div style={{ ...cell, opacity: 0.7, textAlign: "right" }}>
        {entry.lineNumber}
      </div>
      <div style={{ ...cell, opacity: 0.85 }}>
        {entry.timestampDisplay ?? "—"}
      </div>
      <div style={cell} title={entry.component ?? undefined}>
        {entry.component ?? ""}
      </div>
      <div style={{ ...cell, opacity: 0.85, textAlign: "right" }}>
        {entry.threadDisplay ?? (entry.thread != null ? String(entry.thread) : "")}
      </div>
      <div style={cell} title={entry.message}>
        {searchNeedle
          ? renderHighlighted(entry.message, searchNeedle, palette.highlightDefault)
          : entry.message}
      </div>
    </div>
  );
}

function SeverityDot({
  severity,
  palette,
}: {
  severity: Severity;
  palette: ReturnType<typeof useTheme>["theme"]["severityPalette"];
}) {
  const color =
    severity === "Error"
      ? palette.error.text
      : severity === "Warning"
        ? palette.warning.text
        : palette.info.text;
  // Info = hollow ring; Warn/Error = filled so they pop.
  const filled = severity !== "Info";
  return (
    <span
      aria-hidden
      style={{
        display: "inline-block",
        width: 8,
        height: 8,
        borderRadius: "50%",
        background: filled ? color : "transparent",
        border: `1.5px solid ${color}`,
      }}
    />
  );
}

function paletteForSeverity(
  severity: Severity,
  palette: ReturnType<typeof useTheme>["theme"]["severityPalette"],
) {
  if (severity === "Error") return palette.error;
  if (severity === "Warning") return palette.warning;
  return palette.info;
}

/* ───────────────────────── Detail pane ───────────────────────── */

interface DetailPaneProps {
  entry: LogEntry | null;
  palette: ReturnType<typeof useTheme>["theme"]["severityPalette"];
}

function DetailPane({ entry, palette }: DetailPaneProps) {
  if (!entry) {
    return (
      <div
        style={{
          height: 120,
          minHeight: 120,
          borderTop: `1px solid ${tokens.colorNeutralStroke1}`,
          background: tokens.colorNeutralBackground2,
          color: tokens.colorNeutralForeground3,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontFamily: LOG_UI_FONT_FAMILY,
          fontSize: tokens.fontSizeBase200,
          fontStyle: "italic",
        }}
      >
        Select a row to see full detail
      </div>
    );
  }
  const sev = paletteForSeverity(entry.severity, palette);
  const meta: React.CSSProperties = {
    fontFamily: LOG_UI_FONT_FAMILY,
    fontSize: tokens.fontSizeBase100,
    color: tokens.colorNeutralForeground3,
    textTransform: "uppercase",
    letterSpacing: "0.04em",
    marginBottom: 2,
  };
  const val: React.CSSProperties = {
    fontFamily: LOG_MONOSPACE_FONT_FAMILY,
    fontSize: tokens.fontSizeBase200,
    color: tokens.colorNeutralForeground1,
  };
  return (
    <div
      style={{
        height: 160,
        minHeight: 160,
        borderTop: `1px solid ${tokens.colorNeutralStroke1}`,
        background: tokens.colorNeutralBackground2,
        padding: "8px 12px",
        overflow: "auto",
        display: "grid",
        gridTemplateColumns: "max-content max-content max-content max-content max-content 1fr",
        gridTemplateRows: "auto 1fr",
        columnGap: 20,
        rowGap: 4,
      }}
    >
      <div style={meta}>Severity</div>
      <div style={meta}>Line</div>
      <div style={meta}>Timestamp</div>
      <div style={meta}>Component</div>
      <div style={meta}>Thread</div>
      <div style={meta}>Source</div>

      <div
        style={{
          ...val,
          padding: "0 6px",
          borderRadius: tokens.borderRadiusSmall,
          background: sev.background,
          color: sev.text,
          fontWeight: 600,
          justifySelf: "start",
        }}
      >
        {entry.severity}
      </div>
      <div style={val}>{entry.lineNumber}</div>
      <div style={val}>{entry.timestampDisplay ?? "—"}</div>
      <div style={val}>{entry.component ?? "—"}</div>
      <div style={val}>
        {entry.threadDisplay ?? (entry.thread != null ? String(entry.thread) : "—")}
      </div>
      <div style={val}>{entry.sourceFile ?? entry.filePath ?? "—"}</div>

      <div
        style={{
          gridColumn: "1 / -1",
          marginTop: 6,
          paddingTop: 6,
          borderTop: `1px dashed ${tokens.colorNeutralStroke2}`,
          ...val,
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        {entry.message}
      </div>
    </div>
  );
}

/* ───────────────────────── Highlight helpers ───────────────────────── */

function renderHighlighted(
  text: string,
  needle: string,
  highlightColor: string,
): React.ReactNode {
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
          background: highlightColor,
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
