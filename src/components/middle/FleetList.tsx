// Placeholder. Real implementation lands in Task 15; UI stub so MiddlePane
// can import and render something without crashing.

import { theme } from "../../lib/theme";
import { useBridgeState } from "../../lib/bridge-state";

export function FleetList() {
  const { state } = useBridgeState();
  return (
    <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
      {state.fleetResult
        ? `fleet mode — ${state.fleetResult.matches} matches across ${state.fleetResult.devices} devices`
        : "fleet mode — run a query to see matches across the fleet"}
    </div>
  );
}
