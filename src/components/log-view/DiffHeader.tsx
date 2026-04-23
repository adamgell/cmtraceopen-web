// Ported from desktop (src/components/log-view/DiffHeader.tsx).
//
// Web adaptation:
//   - Zustand store reads (`useLogStore`) replaced with explicit props; the
//     caller owns diff state + the mode toggle / close callbacks.
//   - `diffFileBaseName` (from desktop's ../../lib/diff-entries) is inlined
//     here as a trivial basename helper so this file has no desktop-only
//     lib dependencies.
//   - Uses tokens + shared font constants only — no hardcoded colors.

import { tokens } from "@fluentui/react-components";
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
        backgroundColor: tokens.colorNeutralBackground3,
        borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: "11px",
        flexShrink: 0,
      }}
    >
      <span style={{ fontWeight: 600, color: tokens.colorNeutralForeground1 }}>
        Diff: {diffFileBaseName(sourceA.filePath)} vs{" "}
        {diffFileBaseName(sourceB.filePath)}
      </span>

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: tokens.colorNeutralStroke2,
        }}
      />

      <span style={{ color: tokens.colorNeutralForeground3 }}>
        {stats.common} common
      </span>
      <span
        style={{
          color: tokens.colorPaletteGreenForeground1,
          fontWeight: 600,
        }}
      >
        {stats.onlyA} only A
      </span>
      <span
        style={{
          color: tokens.colorPaletteRedForeground1,
          fontWeight: 600,
        }}
      >
        {stats.onlyB} only B
      </span>

      <div
        style={{
          width: "1px",
          height: "16px",
          backgroundColor: tokens.colorNeutralStroke2,
        }}
      />

      <div style={{ display: "flex" }}>
        <button
          type="button"
          onClick={() => onChangeDisplayMode("side-by-side")}
          style={{
            fontSize: "10px",
            padding: "2px 8px",
            border: `1px solid ${displayMode === "side-by-side" ? tokens.colorBrandStroke1 : tokens.colorNeutralStroke2}`,
            backgroundColor:
              displayMode === "side-by-side"
                ? tokens.colorBrandBackground2
                : tokens.colorNeutralBackground1,
            color:
              displayMode === "side-by-side"
                ? tokens.colorBrandForeground1
                : tokens.colorNeutralForeground3,
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
            border: `1px solid ${displayMode === "unified" ? tokens.colorBrandStroke1 : tokens.colorNeutralStroke2}`,
            backgroundColor:
              displayMode === "unified"
                ? tokens.colorBrandBackground2
                : tokens.colorNeutralBackground1,
            color:
              displayMode === "unified"
                ? tokens.colorBrandForeground1
                : tokens.colorNeutralForeground3,
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
          border: `1px solid ${tokens.colorNeutralStroke2}`,
          borderRadius: "3px",
          backgroundColor: tokens.colorNeutralBackground1,
          color: tokens.colorNeutralForeground1,
          cursor: "pointer",
        }}
      >
        Close Diff
      </button>
    </div>
  );
}
