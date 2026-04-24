// Keyboard-shortcut cheat sheet. Mounted at the CommandBridge root via a
// Fragment so its fixed positioning is not clipped by any shell grid parent.
// The backdrop itself dismisses the overlay; the modal body stops propagation
// so interacting with its contents (copying a shortcut label, etc.) does NOT
// close it. Rendering returns null when `open` is false.

import { theme } from "../../lib/theme";

const SHORTCUTS: { keys: string; label: string }[] = [
  { keys: "⌘/", label: "focus query bar" },
  { keys: "⌘B", label: "toggle rail (collapse/expand)" },
  { keys: "⌘K", label: "jump to device search" },
  { keys: "⌘↑ / ⌘↓", label: "previous / next file in session" },
  { keys: "J / K", label: "row navigation in log grid" },
  { keys: "/", label: "focus log-grid search" },
  { keys: "Enter", label: "open row detail" },
  { keys: "?", label: "this help overlay" },
  { keys: "Esc", label: "close dropdown / overlay / dismiss" },
];

interface Props {
  open: boolean;
  onClose: () => void;
}

export function HelpOverlay({ open, onClose }: Props) {
  if (!open) return null;
  return (
    <div
      data-testid="help-backdrop"
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.55)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 100,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: theme.bg,
          border: `1px solid ${theme.border}`,
          borderRadius: 8,
          padding: "1.25rem 1.5rem",
          minWidth: "360px",
          fontFamily: theme.font.mono,
          fontSize: "0.78rem",
          color: theme.text,
        }}
      >
        <h2
          style={{
            margin: "0 0 0.75rem",
            color: theme.accent,
            fontSize: "0.7rem",
            letterSpacing: "0.15em",
            textTransform: "uppercase",
          }}
        >
          Keyboard shortcuts
        </h2>
        <dl
          style={{
            display: "grid",
            gridTemplateColumns: "110px 1fr",
            gap: "0.3rem 1rem",
            margin: 0,
          }}
        >
          {SHORTCUTS.map(({ keys, label }) => (
            <div key={label} style={{ display: "contents" }}>
              <dt style={{ color: theme.accent }}>{keys}</dt>
              <dd style={{ margin: 0 }}>{label}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
