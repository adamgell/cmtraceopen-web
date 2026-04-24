// StatusBar — bottom strip of the right pane. Shows rows rendered / page
// limit, total entry count (compacted), colored warn/err tallies, and a
// right-aligned shortcut hint block. Keeps the dense command-bridge aesthetic:
// small mono font, subtle dividers as middle-dots.

import { theme } from "../../lib/theme";

function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

interface Props {
  rendered: number;
  limit: number;
  total: number;
  warnCount: number;
  errCount: number;
}

export function StatusBar({ rendered, limit, total, warnCount, errCount }: Props) {
  return (
    <div
      data-testid="status-bar"
      style={{
        borderTop: `1px solid ${theme.border}`,
        padding: "0.3rem 0.7rem",
        fontFamily: theme.font.mono,
        fontSize: "0.6rem",
        color: theme.textDim,
        display: "flex",
        gap: "1rem",
      }}
    >
      <span>
        rows <b style={{ color: theme.text }}>{rendered} / {limit}</b>
      </span>
      <span>· {compact(total)} total</span>
      <span>
        · <span style={{ color: theme.pill.okFallbacks.fg }}>{warnCount} warn</span>{" "}
        <span style={{ color: theme.pill.failed.fg }}>{errCount} err</span>
      </span>
      <span style={{ marginLeft: "auto" }}>
        <b style={{ color: theme.accent }}>⌘↑↓</b> next file ·{" "}
        <b style={{ color: theme.accent }}>J/K</b> row ·{" "}
        <b style={{ color: theme.accent }}>/</b> find
      </span>
    </div>
  );
}
