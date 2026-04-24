// Top-level shell for the command-bridge UI. See
// docs/superpowers/specs/2026-04-24-viewer-command-bridge-design.md for the
// region layout. Each region is a test-id'd div in v1 — real content lands
// in Tasks 4-17.

import { useState } from "react";
import { BridgeStateProvider, useBridgeState } from "../../lib/bridge-state";
import { runKqlStub } from "../../lib/kql-executor-stub";
import { useShortcut } from "../../lib/keyboard-shortcuts";
import { theme } from "../../lib/theme";
import { HelpOverlay } from "../overlays/HelpOverlay";
import { LocalOverlay } from "../overlays/LocalOverlay";
import { DeviceRail } from "../rail/DeviceRail";
import { MiddlePane } from "../middle/MiddlePane";
import { LogViewer } from "../right/LogViewer";
import { Banner } from "./Banner";
import { KqlBar } from "./KqlBar";
import { TopHeader } from "./TopHeader";
import { ResultStrip } from "./ResultStrip";

export function CommandBridge() {
  return (
    <BridgeStateProvider>
      <BridgeInner />
    </BridgeStateProvider>
  );
}

function BridgeInner() {
  const { state, dispatch } = useBridgeState();
  const railWidth = state.railExpanded ? "220px" : "56px";
  const [helpOpen, setHelpOpen] = useState(false);
  const [localOpen, setLocalOpen] = useState(false);

  // ⌘B — toggle rail (collapse/expand)
  useShortcut({ key: "b", meta: true }, (e) => {
    e.preventDefault();
    dispatch({ type: "toggle-rail" });
  });
  // ⌘/ — focus the KQL query bar (id="kql-input" lives on the KqlBar input)
  useShortcut({ key: "/", meta: true }, (e) => {
    e.preventDefault();
    document.getElementById("kql-input")?.focus();
  });
  // ⌘O — open LocalMode overlay (file-open). The overlay's own drop-zone
  // handles the actual file — this shortcut just reveals the UI.
  useShortcut({ key: "o", meta: true }, (e) => {
    e.preventDefault();
    setLocalOpen(true);
  });
  // ? — open help overlay. Bail if the user is typing in an input so a literal
  // `?` inside the query bar isn't stolen.
  useShortcut({ key: "?" }, (e) => {
    if (document.activeElement?.tagName === "INPUT") return;
    e.preventDefault();
    setHelpOpen(true);
  });
  // Esc — close help overlay. (Dropdown-close already lives on the KqlBar.)
  useShortcut({ key: "Escape" }, () => setHelpOpen(false));

  return (
    <>
      <div
        onDragOver={(e) => {
          // Must prevent default so the browser accepts the subsequent drop.
          e.preventDefault();
        }}
        onDrop={(e) => {
          const file = e.dataTransfer.files?.[0];
          if (!file) return;
          const name = file.name.toLowerCase();
          if (!/\.(log|cmtlog|txt)$/.test(name)) return;
          e.preventDefault();
          setLocalOpen(true);
          // LocalMode's own drop-zone handles the actual file — we just
          // reveal the overlay here. Passing the file directly would need
          // a new prop on LocalMode; for v1 the overlay's first render
          // picks up the user's next drop inside its own DropZone.
        }}
        style={{
          display: "grid",
          gridTemplateRows: "auto auto auto 1fr",
          height: "100vh",
          background: theme.bg,
          color: theme.text,
          fontFamily: theme.font.ui,
        }}
      >
        <TopHeader onHelp={() => setHelpOpen(true)} />
        {/* Wrap KqlBar + ResultStrip in a testid'd div so the existing
            CommandBridge test continues to find the kql-bar region without
            coupling KqlBar to the test. onRun writes the query + a stubbed
            summary into bridge state; ResultStrip renders under the bar when
            state.fleetResult is non-null. */}
        <div data-testid="kql-bar">
          <KqlBar
            onRun={(q) => {
              dispatch({ type: "set-fleet-query", query: q });
              dispatch({ type: "set-fleet-result", result: runKqlStub(q) });
            }}
          />
          <ResultStrip />
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
      <HelpOverlay open={helpOpen} onClose={() => setHelpOpen(false)} />
      <LocalOverlay open={localOpen} onClose={() => setLocalOpen(false)} />
    </>
  );
}
