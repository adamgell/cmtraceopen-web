// Dense 6-column virtualized log grid. Column widths + row padding are
// spec-locked (§6 of 2026-04-24-viewer-command-bridge-design.md).
//
// Virtualization: @tanstack/react-virtual. Target row height is ~18px at
// font-size 0.7rem + line-height 1.28 + padding 0.12rem top/bottom.

import { useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { LogEntry } from "../../lib/log-types";
import { theme } from "../../lib/theme";

const COL_TEMPLATE = "22px 52px 156px 130px 56px 1fr";
const COL_GAP = "0.55rem";

function severityGlyph(sev: string): string {
  switch (sev) {
    case "Warning": return "⚠";
    case "Error": return "✖";
    default: return "·";
  }
}

function severityLabel(sev: string): string {
  switch (sev) {
    case "Warning": return "WARN";
    case "Error": return "ERROR";
    default: return "INFO";
  }
}

function severityColor(sev: string): string {
  switch (sev) {
    case "Warning": return theme.pill.okFallbacks.fg;
    case "Error": return theme.pill.failed.fg;
    default: return theme.textDim;
  }
}

function rowBackground(sev: string, zebra: boolean): string {
  if (sev === "Error") return theme.rowTint.error;
  if (sev === "Warning") return theme.rowTint.warning;
  return zebra ? theme.surfaceAlt : "transparent";
}

interface Props {
  entries: LogEntry[];
  onOpenRow?: (entry: LogEntry) => void;
}

export function EntryGrid({ entries, onOpenRow }: Props) {
  const parentRef = useRef<HTMLDivElement | null>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const virt = useVirtualizer({
    count: entries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 18,
    overscan: 20,
  });

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <style>{`[data-grid-row]:focus-visible { outline: 1px solid ${theme.accent}; outline-offset: -1px; }`}</style>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: COL_TEMPLATE,
          gap: COL_GAP,
          padding: "0.22rem 0.7rem",
          borderBottom: `1px solid ${theme.border}`,
          background: theme.surface,
          color: theme.textDim,
          fontFamily: theme.font.mono,
          fontSize: "0.58rem",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
        }}
      >
        <span />
        <span>LINE</span>
        <span>TIMESTAMP</span>
        <span>COMPONENT</span>
        <span>SEV</span>
        <span>MESSAGE</span>
      </div>
      <div
        ref={parentRef}
        style={{
          flex: 1,
          overflow: "auto",
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
          lineHeight: 1.28,
        }}
      >
        {entries.length === 0 && (
          <div style={{ padding: "0.7rem", color: theme.textDim, fontSize: "0.65rem" }}>
            no entries to render
          </div>
        )}
        <div style={{ height: virt.getTotalSize(), position: "relative" }}>
          {virt.getVirtualItems().map((v) => {
            const e = entries[v.index];
            if (!e) return null;
            const glyph = severityGlyph(e.severity);
            const label = severityLabel(e.severity);
            const color = severityColor(e.severity);
            const isHovered = hoveredIndex === v.index;
            const bg = isHovered ? theme.hoverBg : rowBackground(e.severity, v.index % 2 === 1);
            return (
              <div
                key={e.id}
                data-grid-row=""
                role="button"
                tabIndex={0}
                onClick={() => onOpenRow?.(e)}
                onKeyDown={(ev) => {
                  if (ev.key === "Enter") onOpenRow?.(e);
                }}
                onMouseEnter={() => setHoveredIndex(v.index)}
                onMouseLeave={() => setHoveredIndex(null)}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  transform: `translateY(${v.start}px)`,
                  width: "100%",
                  display: "grid",
                  gridTemplateColumns: COL_TEMPLATE,
                  gap: COL_GAP,
                  padding: "0.12rem 0.7rem",
                  borderBottom: `1px solid ${theme.surfaceAlt}`,
                  color: theme.text,
                  background: bg,
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  cursor: onOpenRow ? "pointer" : "default",
                  outline: "none",
                }}
              >
                <span style={{ color }}>{glyph}</span>
                <span style={{ color: theme.textFainter, textAlign: "right" }}>{e.lineNumber}</span>
                <span style={{ color: theme.textDim }}>{e.timestampDisplay ?? "—"}</span>
                <span style={{ color: theme.accent, overflow: "hidden", textOverflow: "ellipsis" }}>
                  {e.component ?? ""}
                </span>
                <span style={{ color }}>{label}</span>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{e.message}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
