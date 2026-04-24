// Top header strip — brand on the left, external links on the right.
// Sits above the KQL bar. Links open in new tabs.

import { theme } from "../../lib/theme";
import { UserChip } from "./UserChip";

const REPO_URL = "https://github.com/adamgell/cmtraceopen-web";
const DOCS_URL = "https://github.com/adamgell/cmtraceopen-web/tree/main/docs";

interface Props {
  /** Open the keyboard-shortcut help overlay. */
  onHelp: () => void;
  /** Project version (shown next to the brand). */
  version?: string;
}

export function TopHeader({ onHelp, version = "0.1.0" }: Props) {
  return (
    <div
      data-testid="top-header"
      style={{
        display: "flex",
        alignItems: "center",
        gap: "1rem",
        padding: "0.35rem 0.9rem",
        background: theme.bgDeep,
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: theme.font.ui,
      }}
    >
      <span
        style={{
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
          letterSpacing: "0.2em",
          color: theme.accent,
          fontWeight: 700,
        }}
      >
        CMTRACE·OPEN
      </span>
      <span style={{ fontFamily: theme.font.mono, fontSize: "0.6rem", color: theme.textDim }}>
        v{version}
      </span>
      <nav style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: "0.25rem" }}>
        <NavLink href="/" label="Status" title="api-server status page" />
        <NavLink href={DOCS_URL} label="Docs" />
        <NavLink href={REPO_URL} label="GitHub" />
        <button
          type="button"
          onClick={onHelp}
          style={{
            all: "unset",
            cursor: "pointer",
            padding: "0.2rem 0.55rem",
            fontFamily: theme.font.mono,
            fontSize: "0.65rem",
            color: theme.textDim,
            borderRadius: 3,
          }}
          onMouseEnter={(e) => (e.currentTarget.style.color = theme.accent)}
          onMouseLeave={(e) => (e.currentTarget.style.color = theme.textDim)}
          title="Keyboard shortcuts (press ? anywhere)"
        >
          Help <span style={{ color: theme.textFainter }}>·</span> ?
        </button>
        <span style={{ width: 1, height: 18, background: theme.border, marginLeft: "0.35rem", marginRight: "0.35rem" }} />
        <UserChip />
      </nav>
    </div>
  );
}

function NavLink({ href, label, title }: { href: string; label: string; title?: string }) {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      title={title}
      style={{
        padding: "0.2rem 0.55rem",
        fontFamily: theme.font.mono,
        fontSize: "0.65rem",
        color: theme.textDim,
        textDecoration: "none",
        borderRadius: 3,
      }}
      onMouseEnter={(e) => (e.currentTarget.style.color = theme.accent)}
      onMouseLeave={(e) => (e.currentTarget.style.color = theme.textDim)}
    >
      {label}
    </a>
  );
}
