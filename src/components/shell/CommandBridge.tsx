// Top-level shell for the command-bridge UI. See
// docs/superpowers/specs/2026-04-24-viewer-command-bridge-design.md for the
// region layout. Each region is a test-id'd div in v1 — real content lands
// in Tasks 4-17.

import { BridgeStateProvider, useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";

export function CommandBridge() {
  return (
    <BridgeStateProvider>
      <BridgeInner />
    </BridgeStateProvider>
  );
}

function BridgeInner() {
  const { state } = useBridgeState();
  const railWidth = state.railExpanded ? "220px" : "56px";
  return (
    <div
      style={{
        display: "grid",
        gridTemplateRows: "auto auto 1fr",
        height: "100vh",
        background: theme.bg,
        color: theme.text,
        fontFamily: theme.font.ui,
      }}
    >
      <div data-testid="kql-bar" style={{ padding: "0.5rem 0.75rem", borderBottom: `1px solid ${theme.border}` }}>
        <span style={{ color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.72rem" }}>
          KQL bar placeholder
        </span>
      </div>
      <div
        data-testid="banner"
        style={{
          padding: "0.45rem 0.9rem",
          borderBottom: `1px solid ${theme.border}`,
          background: theme.bg,
          backgroundImage: theme.pattern.dots,
        }}
      >
        <span style={{ color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.72rem" }}>
          banner placeholder
        </span>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: `${railWidth} 220px 1fr`, minHeight: 0 }}>
        <div data-testid="rail" style={{ width: railWidth, borderRight: `1px solid ${theme.border}`, overflow: "auto" }}>
          <span style={{ color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.6rem", padding: "0.5rem", display: "block" }}>rail</span>
        </div>
        <div data-testid="middle-pane" style={{ borderRight: `1px solid ${theme.border}`, overflow: "auto" }}>
          <span style={{ color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.6rem", padding: "0.5rem", display: "block" }}>middle</span>
        </div>
        <div data-testid="right-pane" style={{ display: "grid", gridTemplateRows: "1fr auto", minHeight: 0 }}>
          <div style={{ overflow: "auto", padding: "0.5rem", fontFamily: theme.font.mono, color: theme.textDim, fontSize: "0.7rem" }}>
            right-pane content
          </div>
          <div
            data-testid="status-bar"
            style={{
              borderTop: `1px solid ${theme.border}`,
              padding: "0.3rem 0.7rem",
              fontFamily: theme.font.mono,
              fontSize: "0.6rem",
              color: theme.textDim,
            }}
          >
            status bar
          </div>
        </div>
      </div>
    </div>
  );
}
