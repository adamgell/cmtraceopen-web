// FilterBar — controls above the entry grid. Severity pills (Info/Warn/Error)
// toggle the corresponding rows in LogViewer's filter set; message search and
// component search are case-insensitive substring filters. The rendered/total
// counter uses compact notation (k/M) to fit in the dense layout.

import { theme } from "../../lib/theme";

export interface Filters {
  info: boolean;
  warn: boolean;
  error: boolean;
  search: string;
  component: string;
}

export interface Totals {
  rendered: number;
  total: number;
}

function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

interface Props {
  filters: Filters;
  totals: Totals;
  onChange: (next: Filters) => void;
}

export function FilterBar({ filters, totals, onChange }: Props) {
  const pill = (key: "info" | "warn" | "error", label: string) => {
    const on = filters[key];
    const severityBg = key === "error" ? theme.pill.failed.bg : theme.accentBg;
    const severityFg = key === "error" ? theme.pill.failed.fg : theme.accent;
    return (
      <button
        type="button"
        onClick={() => onChange({ ...filters, [key]: !on })}
        style={{
          all: "unset",
          padding: "0.15rem 0.45rem",
          border: `1px solid ${on ? severityFg : theme.border}`,
          borderRadius: 3,
          color: on ? severityFg : theme.textDim,
          background: on ? severityBg : "transparent",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          cursor: "pointer",
        }}
      >
        {label}
      </button>
    );
  };
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.3rem",
        padding: "0.3rem 0.7rem",
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: theme.font.mono,
        fontSize: "0.65rem",
      }}
    >
      {pill("info", "Info")}
      {pill("warn", "Warn")}
      {pill("error", "Error")}
      <input
        value={filters.search}
        onChange={(e) => onChange({ ...filters, search: e.target.value })}
        placeholder="search message…"
        style={{
          flex: 1,
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.text,
          padding: "0.18rem 0.4rem",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          borderRadius: 3,
        }}
      />
      <input
        value={filters.component}
        onChange={(e) => onChange({ ...filters, component: e.target.value })}
        placeholder="Component…"
        style={{
          width: "120px",
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.text,
          padding: "0.18rem 0.4rem",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          borderRadius: 3,
        }}
      />
      <span style={{ color: theme.textDim, marginLeft: "0.3rem" }}>
        {compact(totals.rendered)} / {compact(totals.total)}
      </span>
    </div>
  );
}
