// Full-screen overlay that reuses the existing `LocalMode` component for
// drag-drop / ⌘O log ingestion. Mounted at the CommandBridge root via a
// Fragment so it escapes the shell grid. The LocalMode component itself
// carries its own drop-zone + Fluent UI dependencies — this overlay just
// frames it and handles Esc-to-close. Task 19 will restyle LocalMode to
// match theme tokens.

import { useEffect } from "react";
import { LocalMode } from "../LocalMode";
import { theme } from "../../lib/theme";

interface Props {
  open: boolean;
  onClose: () => void;
}

export function LocalOverlay({ open, onClose }: Props) {
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: theme.bg,
        zIndex: 200,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <header
        style={{
          padding: "0.5rem 0.75rem",
          borderBottom: `1px solid ${theme.border}`,
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
        }}
      >
        <span style={{ color: theme.accent }}>LOCAL · FILE</span>
        <button
          type="button"
          onClick={onClose}
          style={{ all: "unset", color: theme.textDim, cursor: "pointer" }}
        >
          Esc · close
        </button>
      </header>
      <div style={{ flex: 1, overflow: "auto" }}>
        <LocalMode />
      </div>
    </div>
  );
}
