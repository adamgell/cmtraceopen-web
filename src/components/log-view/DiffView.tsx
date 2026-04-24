// Ported from desktop (src/components/log-view/DiffView.tsx).
//
// Web adaptation:
//   - Zustand store reads (`useLogStore`, `useUiStore`) replaced with
//     explicit props; the parent supplies the diff state, the selected
//     entry id, and callbacks. This matches the web viewer's general
//     "props, not stores" posture (see EntryList / FilterBar).
//   - Imports from desktop-only libs (`../../lib/diff-entries`,
//     `../../lib/date-time-format`) are not available in the web repo.
//     Minimal replacements are inlined:
//       * `EntryClassification` — kept as an exported type alias.
//       * `diffFileBaseName` — path basename helper.
//       * `formatDisplayDateTime` — timestamp -> short local string,
//         preferring the pre-formatted `timestampDisplay` when present.
//   - Still uses `@tanstack/react-virtual` (already a web dependency).
//   - Still uses `@fluentui/react-components` tokens + the shared font
//     constants from `lib/log-accessibility.ts`. No hardcoded hex colors.
//
// TODO(web-port): once the web viewer grows a real diff feature, lift
// `DiffState` into `lib/diff-entries.ts` (mirroring desktop) and drop the
// local definition here.

import { useMemo, useRef } from "react";
import { theme } from "../../lib/theme";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  LOG_MONOSPACE_FONT_FAMILY,
  LOG_UI_FONT_FAMILY,
  getLogListMetrics,
  DEFAULT_LOG_LIST_FONT_SIZE,
} from "../../lib/log-accessibility";
import type { LogEntry } from "../../lib/log-types";
import { DiffHeader, type DiffDisplayMode } from "./DiffHeader";

// ---------------------------------------------------------------------------
// Types

export type EntryClassification = "common" | "only-a" | "only-b";

export interface DiffViewSource {
  filePath: string;
}

export interface DiffViewStats {
  common: number;
  onlyA: number;
  onlyB: number;
}

export interface DiffViewState {
  sourceA: DiffViewSource;
  sourceB: DiffViewSource;
  entriesA: LogEntry[];
  entriesB: LogEntry[];
  /** Map from `LogEntry.id` to classification. */
  entryClassification: Map<number, EntryClassification>;
  stats: DiffViewStats;
  displayMode: DiffDisplayMode;
}

export interface DiffViewProps {
  diffState: DiffViewState;
  selectedId: number | null;
  onSelect: (id: number | null) => void;
  onChangeDisplayMode: (mode: DiffDisplayMode) => void;
  onClose: () => void;
  /** Defaults to `DEFAULT_LOG_LIST_FONT_SIZE`. */
  logListFontSize?: number;
}

// ---------------------------------------------------------------------------
// Local helpers (inlined replacements for desktop-only lib imports).

function diffFileBaseName(filePath: string): string {
  if (!filePath) return "";
  const idx = Math.max(filePath.lastIndexOf("/"), filePath.lastIndexOf("\\"));
  return idx >= 0 ? filePath.slice(idx + 1) : filePath;
}

function formatDisplayDateTime(
  value: string | number | undefined,
): string | undefined {
  if (value == null) return undefined;
  if (typeof value === "string") {
    // Already display-formatted by the parser — trust it verbatim.
    return value;
  }
  const d = new Date(value);
  if (!Number.isFinite(d.getTime())) return undefined;
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

const CLASS_COLORS: Record<
  EntryClassification,
  { bg: string; border: string }
> = {
  common: { bg: "transparent", border: "transparent" },
  "only-a": {
    bg: theme.accentBg,
    border: theme.accent,
  },
  "only-b": {
    bg: theme.pill.failed.bg,
    border: theme.pill.failed.fg,
  },
};

// ---------------------------------------------------------------------------

export function DiffView({
  diffState,
  selectedId,
  onSelect,
  onChangeDisplayMode,
  onClose,
  logListFontSize = DEFAULT_LOG_LIST_FONT_SIZE,
}: DiffViewProps) {
  const metrics = useMemo(
    () => getLogListMetrics(logListFontSize),
    [logListFontSize],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        fontFamily: LOG_UI_FONT_FAMILY,
      }}
    >
      <DiffHeader
        sourceA={diffState.sourceA}
        sourceB={diffState.sourceB}
        stats={diffState.stats}
        displayMode={diffState.displayMode}
        onChangeDisplayMode={onChangeDisplayMode}
        onClose={onClose}
      />
      {diffState.displayMode === "side-by-side" ? (
        <SideBySideView
          diffState={diffState}
          metrics={metrics}
          selectedId={selectedId}
          onSelect={onSelect}
        />
      ) : (
        <UnifiedView
          diffState={diffState}
          metrics={metrics}
          selectedId={selectedId}
          onSelect={onSelect}
        />
      )}
    </div>
  );
}

function SideBySideView({
  diffState,
  metrics,
  selectedId,
  onSelect,
}: {
  diffState: DiffViewState;
  metrics: ReturnType<typeof getLogListMetrics>;
  selectedId: number | null;
  onSelect: (id: number | null) => void;
}) {
  const parentRefA = useRef<HTMLDivElement>(null);
  const parentRefB = useRef<HTMLDivElement>(null);
  const isSyncingRef = useRef(false);
  const rowHeight = metrics.rowHeight;

  const handleScrollA = () => {
    if (isSyncingRef.current || !parentRefA.current || !parentRefB.current)
      return;
    isSyncingRef.current = true;
    parentRefB.current.scrollTop = parentRefA.current.scrollTop;
    requestAnimationFrame(() => {
      isSyncingRef.current = false;
    });
  };

  const handleScrollB = () => {
    if (isSyncingRef.current || !parentRefA.current || !parentRefB.current)
      return;
    isSyncingRef.current = true;
    parentRefA.current.scrollTop = parentRefB.current.scrollTop;
    requestAnimationFrame(() => {
      isSyncingRef.current = false;
    });
  };

  const virtualizerA = useVirtualizer({
    count: diffState.entriesA.length,
    getScrollElement: () => parentRefA.current,
    estimateSize: () => rowHeight,
    overscan: 10,
  });

  const virtualizerB = useVirtualizer({
    count: diffState.entriesB.length,
    getScrollElement: () => parentRefB.current,
    estimateSize: () => rowHeight,
    overscan: 10,
  });

  return (
    <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          borderRight: `1px solid ${theme.border}`,
        }}
      >
        <div
          style={{
            padding: "4px 8px",
            fontSize: "11px",
            fontWeight: 600,
            backgroundColor: theme.surfaceAlt,
            borderBottom: `1px solid ${theme.border}`,
            color: theme.accent,
          }}
        >
          A: {diffFileBaseName(diffState.sourceA.filePath)} (
          {diffState.entriesA.length})
        </div>
        <div
          ref={parentRefA}
          onScroll={handleScrollA}
          style={{ flex: 1, overflowY: "auto" }}
        >
          <div
            style={{
              height: `${virtualizerA.getTotalSize()}px`,
              position: "relative",
            }}
          >
            {virtualizerA.getVirtualItems().map((row) => {
              const entry = diffState.entriesA[row.index]!;
              const cls =
                diffState.entryClassification.get(entry.id) ?? "common";
              return (
                <div
                  key={row.key}
                  data-index={row.index}
                  ref={virtualizerA.measureElement}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${row.start}px)`,
                  }}
                >
                  <DiffRow
                    entry={entry}
                    classification={cls}
                    isSelected={entry.id === selectedId}
                    fontSize={metrics.fontSize}
                    onSelect={onSelect}
                  />
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <div style={{ flex: 1, display: "flex", flexDirection: "column" }}>
        <div
          style={{
            padding: "4px 8px",
            fontSize: "11px",
            fontWeight: 600,
            backgroundColor: theme.surfaceAlt,
            borderBottom: `1px solid ${theme.border}`,
            color: theme.pill.failed.fg,
          }}
        >
          B: {diffFileBaseName(diffState.sourceB.filePath)} (
          {diffState.entriesB.length})
        </div>
        <div
          ref={parentRefB}
          onScroll={handleScrollB}
          style={{ flex: 1, overflowY: "auto" }}
        >
          <div
            style={{
              height: `${virtualizerB.getTotalSize()}px`,
              position: "relative",
            }}
          >
            {virtualizerB.getVirtualItems().map((row) => {
              const entry = diffState.entriesB[row.index]!;
              const cls =
                diffState.entryClassification.get(entry.id) ?? "common";
              return (
                <div
                  key={row.key}
                  data-index={row.index}
                  ref={virtualizerB.measureElement}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${row.start}px)`,
                  }}
                >
                  <DiffRow
                    entry={entry}
                    classification={cls}
                    isSelected={entry.id === selectedId}
                    fontSize={metrics.fontSize}
                    onSelect={onSelect}
                  />
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

function UnifiedView({
  diffState,
  metrics,
  selectedId,
  onSelect,
}: {
  diffState: DiffViewState;
  metrics: ReturnType<typeof getLogListMetrics>;
  selectedId: number | null;
  onSelect: (id: number | null) => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const rowHeight = metrics.rowHeight;

  const unifiedEntries = useMemo(() => {
    const all = [
      ...diffState.entriesA.map((e) => ({ entry: e, source: "a" as const })),
      ...diffState.entriesB.map((e) => ({ entry: e, source: "b" as const })),
    ];
    all.sort((x, y) => {
      if (x.entry.timestamp != null && y.entry.timestamp != null) {
        if (x.entry.timestamp !== y.entry.timestamp)
          return x.entry.timestamp - y.entry.timestamp;
      }
      return x.entry.lineNumber - y.entry.lineNumber;
    });
    return all;
  }, [diffState.entriesA, diffState.entriesB]);

  const virtualizer = useVirtualizer({
    count: unifiedEntries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 10,
  });

  return (
    <div ref={parentRef} style={{ flex: 1, overflowY: "auto" }}>
      <div
        style={{
          height: `${virtualizer.getTotalSize()}px`,
          position: "relative",
        }}
      >
        {virtualizer.getVirtualItems().map((row) => {
          const { entry, source } = unifiedEntries[row.index]!;
          const cls = diffState.entryClassification.get(entry.id) ?? "common";
          return (
            <div
              key={row.key}
              data-index={row.index}
              ref={virtualizer.measureElement}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${row.start}px)`,
              }}
            >
              <DiffRow
                entry={entry}
                classification={cls}
                isSelected={entry.id === selectedId}
                fontSize={metrics.fontSize}
                onSelect={onSelect}
                sourceBadge={source.toUpperCase()}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function DiffRow({
  entry,
  classification,
  isSelected,
  fontSize,
  onSelect,
  sourceBadge,
}: {
  entry: LogEntry;
  classification: EntryClassification;
  isSelected: boolean;
  fontSize: number;
  onSelect: (id: number | null) => void;
  sourceBadge?: string;
}) {
  const colors = CLASS_COLORS[classification];
  const monoFont = Math.max(fontSize - 1, 10);

  return (
    <div
      onClick={() => onSelect(isSelected ? null : entry.id)}
      style={{
        display: "flex",
        alignItems: "center",
        gap: "6px",
        padding: "2px 8px",
        fontSize: `${fontSize}px`,
        backgroundColor: isSelected ? theme.accent : colors.bg,
        color: isSelected
          ? theme.bg
          : theme.textPrimary,
        borderLeft: `3px solid ${colors.border}`,
        borderBottom: `1px solid ${theme.border}`,
        cursor: "pointer",
        height: "100%",
        boxSizing: "border-box",
      }}
    >
      {sourceBadge && (
        <span
          style={{
            fontSize: "9px",
            fontWeight: 700,
            padding: "1px 4px",
            borderRadius: "2px",
            backgroundColor:
              classification === "only-a"
                ? theme.accentBg
                : classification === "only-b"
                  ? theme.pill.failed.bg
                  : theme.border,
            color:
              classification === "only-a"
                ? theme.accent
                : classification === "only-b"
                  ? theme.pill.failed.fg
                  : theme.textDim,
            flexShrink: 0,
            width: "16px",
            textAlign: "center",
          }}
        >
          {sourceBadge}
        </span>
      )}
      <span
        style={{
          fontSize: `${monoFont}px`,
          color: isSelected ? "inherit" : theme.textDim,
          fontFamily: LOG_MONOSPACE_FONT_FAMILY,
          flexShrink: 0,
          width: "145px",
        }}
      >
        {formatDisplayDateTime(entry.timestampDisplay ?? entry.timestamp) ??
          "—"}
      </span>
      <span
        style={{
          flex: 1,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          fontFamily: LOG_MONOSPACE_FONT_FAMILY,
          fontSize: `${monoFont}px`,
        }}
      >
        {entry.message}
      </span>
    </div>
  );
}
