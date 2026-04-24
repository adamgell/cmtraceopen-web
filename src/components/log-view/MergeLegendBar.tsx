// Ported from desktop (src/components/log-view/MergeLegendBar.tsx).
//
// Web adaptation:
//   - Zustand store reads (`useLogStore`) replaced with explicit props.
//     Callers pass in the merged-tab state, the correlation window, the
//     auto-correlate flag, the total visible-entry count, and setters.
//   - `fileBaseName` (desktop ../../lib/merge-entries) is inlined as a
//     trivial basename helper.
//   - Uses `@fluentui/react-components` tokens + the shared font constant.
//     The only per-file color that is *not* a Fluent token is the
//     caller-supplied hex from `colorAssignments` (e.g. `#2563eb`) which
//     is a legit data value, not a hardcoded UI color. The desktop fallback
//     `"#888"` is replaced with `tokens.colorNeutralForeground4` so the
//     pill stays theme-aware when a color is missing.
//
// TODO(web-port): lift `MergedTabState` into a shared `lib/merge-entries.ts`
// (mirroring desktop) once the web viewer grows a real merge feature. For
// now it's a local type so this file has no desktop-only lib dependencies.

import { theme } from "../../lib/theme";
import type { LogEntry } from "../../lib/log-types";
import { LOG_UI_FONT_FAMILY } from "../../lib/log-accessibility";

const CORRELATION_WINDOWS = [
  { label: "100ms", value: 100 },
  { label: "500ms", value: 500 },
  { label: "1s", value: 1000 },
  { label: "5s", value: 5000 },
  { label: "10s", value: 10000 },
];

export interface MergedTabState {
  /** Ordered list of source file paths that contribute to the merge. */
  sourceFilePaths: string[];
  /** `filePath -> CSS color` (typically a hex from a stable palette). */
  colorAssignments: Record<string, string>;
  /** `filePath -> true/false`. Missing keys are treated as visible. */
  fileVisibility: Record<string, boolean>;
  /** All merged entries (pre-visibility filtering). */
  mergedEntries: LogEntry[];
}

export interface MergeLegendBarProps {
  mergedTabState: MergedTabState;
  correlationWindowMs: number;
  autoCorrelate: boolean;
  /**
   * Total entries currently visible after applying `fileVisibility` — shown
   * in the "N merged" caption on the right. The caller derives this (same
   * number it feeds the list view) and passes it in.
   */
  visibleEntryCount: number;
  onSetFileVisibility: (filePath: string, visible: boolean) => void;
  onSetAllFileVisibility: (visible: boolean) => void;
  onSetCorrelationWindowMs: (ms: number) => void;
  onSetAutoCorrelate: (on: boolean) => void;
}

function fileBaseName(filePath: string): string {
  if (!filePath) return "";
  const idx = Math.max(filePath.lastIndexOf("/"), filePath.lastIndexOf("\\"));
  return idx >= 0 ? filePath.slice(idx + 1) : filePath;
}

export function MergeLegendBar({
  mergedTabState,
  correlationWindowMs,
  autoCorrelate,
  visibleEntryCount,
  onSetFileVisibility,
  onSetAllFileVisibility,
  onSetCorrelationWindowMs,
  onSetAutoCorrelate,
}: MergeLegendBarProps) {
  const fileCounts: Record<string, number> = {};
  for (const entry of mergedTabState.mergedEntries) {
    fileCounts[entry.filePath] = (fileCounts[entry.filePath] ?? 0) + 1;
  }

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "6px",
        padding: "4px 12px",
        backgroundColor: theme.surfaceAlt,
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: "11px",
        overflowX: "auto",
        scrollbarWidth: "none",
        flexShrink: 0,
      }}
    >
      {mergedTabState.sourceFilePaths.map((fp) => {
        const color =
          mergedTabState.colorAssignments[fp] ??
          theme.textFainter;
        const visible = mergedTabState.fileVisibility[fp] !== false;
        const count = fileCounts[fp] ?? 0;

        return (
          <button
            key={fp}
            type="button"
            onClick={() => onSetFileVisibility(fp, !visible)}
            title={fp}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "4px",
              padding: "2px 8px",
              borderRadius: "12px",
              border: `1px solid ${visible ? color : theme.border}`,
              // Append a low-alpha hex suffix to the caller-provided color
              // for the pill background when visible. If the supplied value
              // isn't a 6-digit hex, the browser just ignores the suffixed
              // form and falls back to a plain color — still readable.
              backgroundColor: visible ? `${color}20` : "transparent",
              color: visible
                ? theme.textPrimary
                : theme.textFainter,
              cursor: "pointer",
              opacity: visible ? 1 : 0.5,
              whiteSpace: "nowrap",
              fontSize: "11px",
              fontFamily: LOG_UI_FONT_FAMILY,
            }}
          >
            <span
              style={{
                width: "8px",
                height: "8px",
                borderRadius: "50%",
                backgroundColor: visible
                  ? color
                  : theme.textFainter,
                flexShrink: 0,
              }}
            />
            <span>{fileBaseName(fp)}</span>
            <span
              style={{
                color: theme.textDim,
                fontWeight: 600,
              }}
            >
              {count}
            </span>
          </button>
        );
      })}

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: theme.border,
          margin: "0 2px",
          flexShrink: 0,
        }}
      />

      <button
        type="button"
        onClick={() => onSetAllFileVisibility(true)}
        style={{
          fontSize: "10px",
          padding: "2px 6px",
          border: `1px solid ${theme.border}`,
          borderRadius: "3px",
          backgroundColor: theme.bg,
          color: theme.textPrimary,
          cursor: "pointer",
        }}
      >
        All
      </button>
      <button
        type="button"
        onClick={() => onSetAllFileVisibility(false)}
        style={{
          fontSize: "10px",
          padding: "2px 6px",
          border: `1px solid ${theme.border}`,
          borderRadius: "3px",
          backgroundColor: theme.bg,
          color: theme.textPrimary,
          cursor: "pointer",
        }}
      >
        None
      </button>

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: theme.border,
          margin: "0 2px",
          flexShrink: 0,
        }}
      />

      <span
        style={{
          color: theme.textDim,
          fontWeight: 600,
          fontSize: "10px",
          textTransform: "uppercase",
        }}
      >
        Correlate:
      </span>
      <select
        value={correlationWindowMs}
        onChange={(e) => onSetCorrelationWindowMs(Number(e.target.value))}
        style={{
          fontSize: "11px",
          padding: "1px 4px",
          border: `1px solid ${theme.border}`,
          borderRadius: "3px",
          backgroundColor: theme.bg,
          color: theme.textPrimary,
        }}
      >
        {CORRELATION_WINDOWS.map((w) => (
          <option key={w.value} value={w.value}>
            {w.label}
          </option>
        ))}
      </select>
      <button
        type="button"
        onClick={() => onSetAutoCorrelate(!autoCorrelate)}
        style={{
          fontSize: "10px",
          padding: "2px 6px",
          border: `1px solid ${autoCorrelate ? theme.accent : theme.border}`,
          borderRadius: "3px",
          backgroundColor: autoCorrelate
            ? theme.accentBg
            : theme.bg,
          color: autoCorrelate
            ? theme.accent
            : theme.textDim,
          cursor: "pointer",
        }}
      >
        Auto
      </button>

      <div
        style={{
          marginLeft: "auto",
          color: theme.textDim,
          flexShrink: 0,
        }}
      >
        {visibleEntryCount} merged
      </div>
    </div>
  );
}
