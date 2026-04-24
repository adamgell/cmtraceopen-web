// Middle pane container — two tabs (DEVICE / FLEET) routing to either the
// SessionTree (device mode) or the FleetList (fleet mode). Mode is held in
// bridge state so the KQL executor (Task 14) can flip modes programmatically.
//
// Tabs and EmptyDevice are file-private; SessionTree and FleetList are
// separate modules so MiddlePane.test.tsx can mock them.

import { useBridgeState, type MiddleMode } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { SessionTree } from "./SessionTree";
import { FleetList } from "./FleetList";

export function MiddlePane() {
  const { state, dispatch } = useBridgeState();
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <Tabs mode={state.middleMode} onPick={(m) => dispatch({ type: "set-middle-mode", mode: m })} />
      <div style={{ flex: 1, minHeight: 0 }}>
        {state.middleMode === "device" ? (
          state.selectedDeviceId ? (
            <SessionTree deviceId={state.selectedDeviceId} />
          ) : (
            <EmptyDevice />
          )
        ) : (
          <FleetList />
        )}
      </div>
    </div>
  );
}

function Tabs({ mode, onPick }: { mode: MiddleMode; onPick: (m: MiddleMode) => void }) {
  const tab = (m: MiddleMode, label: string) => {
    const on = mode === m;
    return (
      <button
        type="button"
        onClick={() => onPick(m)}
        style={{
          all: "unset",
          flex: 1,
          padding: "0.45rem",
          textAlign: "center",
          cursor: "pointer",
          color: on ? theme.accent : theme.textDim,
          fontFamily: theme.font.mono,
          fontSize: "0.62rem",
          letterSpacing: "0.06em",
          textTransform: "uppercase",
          borderBottom: on ? `2px solid ${theme.accent}` : `2px solid transparent`,
        }}
      >
        {label}
      </button>
    );
  };
  return (
    <div style={{ display: "flex", borderBottom: `1px solid ${theme.border}` }}>
      {tab("device", "DEVICE")}
      {tab("fleet", "FLEET")}
    </div>
  );
}

function EmptyDevice() {
  return (
    <div style={{ padding: "1rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
      Pick a device from the rail to load its sessions.
    </div>
  );
}
