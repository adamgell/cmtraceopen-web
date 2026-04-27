// Dark-only theme tokens for the command-bridge shell.
//
// Single source of truth. Every new shell component reads colors / fonts /
// background patterns from here. Fluent UI's own `tokens.*` is deliberately
// NOT used by shell components — we want to fully own the look. LocalMode
// and DiffView keep Fluent until the Task 19 cleanup pass.

export const theme = {
  bg: "#0b0f14",
  bgDeep: "#070a0e",
  surface: "#11161d",
  surfaceAlt: "#0d1218",
  border: "#1f2a36",
  textPrimary: "#f3f7fb",
  text: "#c7d1dd",
  textDim: "#7da2c3",
  textFainter: "#3d4a5a",
  accent: "#5ee3c5",
  accentBg: "#0e2d22",
  hoverBg: "#151c25",
  backdrop: "rgba(0,0,0,0.55)",
  syntax: {
    keyword: "#9a7ef8",
  },
  rowTint: {
    error: "rgba(243,140,140,.08)",
    warning: "rgba(243,195,127,.06)",
  },
  shadow: {
    dropdown: "0 6px 18px rgba(0,0,0,0.4)",
  },
  pill: {
    ok:          { fg: "#5ee3c5", bg: "#0e2d22", dot: "#5ee3c5" },
    okFallbacks: { fg: "#f3c37f", bg: "#3d2e12", dot: "#f3c37f" },
    partial:     { fg: "#e08a45", bg: "#3d2516", dot: "#e08a45" },
    failed:      { fg: "#f38c8c", bg: "#3d1414", dot: "#f38c8c" },
    pending:     { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
    stale:       { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
  },
  font: {
    mono: "ui-monospace, Menlo, Consolas, monospace",
    ui: "ui-sans-serif, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  pattern: {
    dots: "radial-gradient(#1c2735 1px, transparent 1px) 0 0 / 12px 12px",
  },
} as const;

export type PillState = keyof typeof theme.pill;
