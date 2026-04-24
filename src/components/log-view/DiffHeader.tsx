// Ported from desktop (src/components/log-view/DiffHeader.tsx).
//
// Web adaptation:
//   - Zustand store reads (`useLogStore`) replaced with explicit props; the
//     caller owns diff state + the mode toggle / close callbacks.
//   - `diffFileBaseName` (from desktop's ../../lib/diff-entries) is inlined
//     here as a trivial basename helper so this file has no desktop-only
//     lib dependencies.
//   - Uses tokens + shared font constants only — no hardcoded colors.

import { theme } from "../../lib/theme";
import { LOG_UI_FONT_FAMILY } from "../../lib/log-accessibility";

export type DiffDisplayMode = "side-by-side" | "unified";

export interface DiffHeaderSource {
  filePath: string;
}

export interface DiffHeaderStats {
  common: number;
  onlyA: number;
  onlyB: number;
}

export interface DiffHeaderProps {
  sourceA: DiffHeaderSource;
  sourceB: DiffHeaderSource;
  stats: DiffHeaderStats;
  displayMode: DiffDisplayMode;
  onChangeDisplayMode: (mode: DiffDisplayMode) => void;
  onClose: () => void;
}

/** Last path segment; '\\'-or-'/' separated, falls back to the original. */
function diffFileBaseName(filePath: string): string {
  if (!filePath) return "";
  const idx = Math.max(filePath.lastIndexOf("/"), filePath.lastIndexOf("\\"));
  return idx >= 0 ? filePath.slice(idx + 1) : filePath;
}

export function DiffHeader({
  sourceA,
  sourceB,
  stats,
  displayMode,
  onChangeDisplayMode,
  onClose,
}: DiffHeaderProps) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "10px",
        padding: "6px 12px",
        backgroundColor: theme.surfaceAlt,
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: "11px",
        flexShrink: 0,
      }}
    >
      <span style={{ fontWeight: 600, color: theme.textPrimary }}>
        Diff: {diffFileBaseName(sourceA.filePath)} vs{" "}
        {diffFileBaseName(sourceB.filePath)}
      </span>

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: theme.border,
        }}
      />

      <span style={{ color: theme.textDim }}>
        {stats.common} common
      </span>
      <span
        style={{
          color: theme.accent,
          fontWeight: 600,
        }}
      >
        {stats.onlyA} only A
      </span>
      <span
        style={{
          color: theme.pill.failed.fg,
          fontWeight: 600,
        }}
      >
        {stats.onlyB} only B
      </span>

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: theme.border,
        }}
      />

      <div style={{ display: "flex" }}>
        <button
          type="button"
          onClick={() => onChangeDisplayMode("side-by-side")}
          style={{
            fontSize: "10px",
            padding: "2px 8px",
            border: `1px solid ${displayMode === "side-by-side" ? theme.accent : theme.border}`,
            backgroundColor:
              displayMode === "side-by-side"
                ? theme.accentBg
                : theme.bg,
            color:
              displayMode === "side-by-side"
                ? theme.accent
                : theme.textDim,
            fontWeight: displayMode === "side-by-side" ? 700 : 500,
            cursor: "pointer",
            borderRadius: "3px 0 0 3px",
            borderRight: "none",
          }}
        >
          Side-by-Side
        </button>
        <button
          type="button"
          onClick={() => onChangeDisplayMode("unified")}
          style={{
            fontSize: "10px",
            padding: "2px 8px",
            border: `1px solid ${displayMode === "unified" ? theme.accent : theme.border}`,
            backgroundColor:
              displayMode === "unified"
                ? theme.accentBg
                : theme.bg,
            color:
              displayMode === "unified"
                ? theme.accent
                : theme.textDim,
            fontWeight: displayMode === "unified" ? 700 : 500,
            cursor: "pointer",
            borderRadius: "0 3px 3px 0",
          }}
        >
          Unified
        </button>
      </div>

      <button
        type="button"
        onClick={onClose}
        style={{
          marginLeft: "auto",
          fontSize: "10px",
          padding: "2px 8px",
          border: `1px solid ${theme.border}`,
          borderRadius: "3px",
          backgroundColor: theme.bg,
          color: theme.textPrimary,
          cursor: "pointer",
        }}
      >
        Close Diff
      </button>
    </div>
  );
}
