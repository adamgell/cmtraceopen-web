import { theme } from "../../lib/theme";
import { useBridgeState } from "../../lib/bridge-state";

export function ResultStrip() {
  const { state, dispatch } = useBridgeState();
  if (!state.fleetResult) return null;
  const { matches, devices, sessions, files, groupBy } = state.fleetResult;
  return (
    <div
      style={{
        background: theme.surfaceAlt,
        padding: "0.35rem 0.75rem",
        display: "flex",
        gap: "1rem",
        alignItems: "center",
        fontFamily: theme.font.mono,
        fontSize: "0.68rem",
        color: theme.text,
        borderBottom: `1px solid ${theme.border}`,
      }}
    >
      <span><b style={{ color: theme.accent }}>{matches}</b> matches</span>
      <span style={{ color: theme.textFainter }}>·</span>
      <span>{devices} devices</span>
      <span style={{ color: theme.textFainter }}>·</span>
      <span>{sessions} sessions</span>
      <span style={{ color: theme.textFainter }}>·</span>
      <span>{files} files</span>
      <span style={{ padding: "0.1rem 0.45rem", borderRadius: 2, background: theme.surface, color: theme.textDim, border: `1px solid ${theme.border}`, fontSize: "0.62rem" }}>
        grouped by {groupBy}
      </span>
      <button
        type="button"
        onClick={() => dispatch({ type: "set-middle-mode", mode: "fleet" })}
        style={{ all: "unset", marginLeft: "auto", color: theme.accent, cursor: "pointer" }}
      >
        open in fleet pane →
      </button>
    </div>
  );
}
