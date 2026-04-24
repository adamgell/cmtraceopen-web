// Slide-out row detail panel. Absolute-positioned inside LogViewer's
// `position: relative` wrapper so it overlays the grid from the right edge.
// Dismiss via Esc (window-level keydown while open) or the header close button.

import { useEffect } from "react";
import type { LogEntry } from "../../lib/log-types";
import { theme } from "../../lib/theme";

interface Props {
  entry: LogEntry | null;
  onClose: () => void;
}

export function RowDetail({ entry, onClose }: Props) {
  useEffect(() => {
    if (!entry) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [entry, onClose]);

  if (!entry) return null;

  return (
    <aside
      style={{
        position: "absolute",
        top: 0,
        right: 0,
        bottom: 0,
        width: "420px",
        background: theme.bgDeep,
        borderLeft: `1px solid ${theme.border}`,
        padding: "0.9rem 1rem",
        overflow: "auto",
        fontFamily: theme.font.mono,
        fontSize: "0.72rem",
        color: theme.text,
      }}
    >
      <header style={{ display: "flex", alignItems: "center", marginBottom: "0.7rem" }}>
        <span
          style={{
            color: theme.accent,
            fontSize: "0.6rem",
            letterSpacing: "0.12em",
            textTransform: "uppercase",
          }}
        >
          Row detail
        </span>
        <button
          type="button"
          onClick={onClose}
          aria-label="close"
          style={{
            all: "unset",
            marginLeft: "auto",
            padding: "0.15rem 0.45rem",
            border: `1px solid ${theme.border}`,
            borderRadius: 3,
            color: theme.textDim,
            cursor: "pointer",
            fontSize: "0.65rem",
          }}
        >
          Esc · close
        </button>
      </header>
      <dl
        style={{
          display: "grid",
          gridTemplateColumns: "100px 1fr",
          gap: "0.25rem 0.7rem",
          margin: 0,
        }}
      >
        <dt style={{ color: theme.textDim }}>line</dt>
        <dd style={{ margin: 0 }}>{entry.lineNumber}</dd>
        <dt style={{ color: theme.textDim }}>timestamp</dt>
        <dd style={{ margin: 0 }}>{entry.timestampDisplay ?? "—"}</dd>
        <dt style={{ color: theme.textDim }}>severity</dt>
        <dd style={{ margin: 0 }}>{entry.severity}</dd>
        <dt style={{ color: theme.textDim }}>component</dt>
        <dd style={{ margin: 0 }}>{entry.component ?? "—"}</dd>
        <dt style={{ color: theme.textDim }}>thread</dt>
        <dd style={{ margin: 0 }}>{entry.threadDisplay ?? "—"}</dd>
      </dl>
      <section style={{ marginTop: "0.9rem" }}>
        <div
          style={{
            color: theme.textDim,
            fontSize: "0.6rem",
            letterSpacing: "0.1em",
            textTransform: "uppercase",
          }}
        >
          Message
        </div>
        <pre
          style={{
            whiteSpace: "pre-wrap",
            margin: "0.3rem 0 0",
            color: theme.textPrimary,
          }}
        >
          {entry.message}
        </pre>
      </section>
    </aside>
  );
}
