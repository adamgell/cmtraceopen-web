// Top-level shell for the command-bridge UI. See
// docs/superpowers/specs/2026-04-24-viewer-command-bridge-design.md for the
// region layout. Each region is a test-id'd div in v1 — real content lands
// in Tasks 4-17.

import { BridgeStateProvider, useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { DeviceRail } from "../rail/DeviceRail";
import { MiddlePane } from "../middle/MiddlePane";
import { LogViewer } from "../right/LogViewer";
import { Banner } from "./Banner";
import { KqlBar } from "./KqlBar";

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
      {/* Wrap KqlBar in a testid'd div so the existing CommandBridge test
          continues to find the kql-bar region without coupling KqlBar to the
          test. onRun is a console.log placeholder — Task 14 wires the real
          executor. */}
      <div data-testid="kql-bar">
        <KqlBar onRun={(q) => { /* wired in Task 14 */ console.log("run", q); }} />
      </div>
      {/* Banner reflects the rail's selected device. Real device data flows in
          later when the rail knows which device is selected (Task 6+). */}
      <Banner device={null} />
      <div style={{ display: "grid", gridTemplateColumns: `${railWidth} 220px 1fr`, minHeight: 0 }}>
        <div data-testid="rail" style={{ borderRight: `1px solid ${theme.border}`, overflow: "hidden" }}>
          <DeviceRail />
        </div>
        <div data-testid="middle-pane" style={{ borderRight: `1px solid ${theme.border}`, overflow: "hidden" }}>
          <MiddlePane />
        </div>
        <div data-testid="right-pane" style={{ display: "grid", gridTemplateRows: "1fr auto", minHeight: 0 }}>
          <div style={{ overflow: "hidden", minHeight: 0 }}>
            <LogViewer />
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
