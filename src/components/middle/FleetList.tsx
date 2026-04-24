// Fleet-mode body of the middle pane. Renders a flat match list derived from
// the stubbed executor's summary (`state.fleetResult`, set by Task 14's
// KQL executor onRun). Real server-side fleet search replaces this in a
// future task; for v1 we surface one row per device with its most recent
// session so the UI shows something plausible that matches the summary's
// match count.
//
// Clicking a row pins the device AND flips the middle pane back to device
// mode — spec-prescribed UX for drilling into a fleet hit.

import { useEffect, useState } from "react";
import { listDevices, listSessions } from "../../lib/api-client";
import { useBridgeState } from "../../lib/bridge-state";
import { theme, type PillState } from "../../lib/theme";

interface FleetRow {
  deviceId: string;
  sessionId: string;
  parseState: string;
  ingestedUtc: string;
}

function pillFor(state: string): PillState {
  switch (state) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    default: return "pending";
  }
}

export function FleetList() {
  const { state, dispatch } = useBridgeState();
  const [rows, setRows] = useState<FleetRow[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const fleet = state.fleetResult;
    if (!fleet) {
      setRows([]);
      return;
    }
    let cancelled = false;
    setLoading(true);
    (async () => {
      // Stub: pull the recent session from every known device and show up
      // to (matches) rows. Real executor replaces this later; for v1 this
      // gives the UI something plausible to render.
      try {
        const devices = await listDevices();
        const enriched: FleetRow[] = [];
        for (const d of devices.items) {
          try {
            const sessions = await listSessions(d.deviceId);
            const top = sessions.items[0];
            if (top) {
              enriched.push({
                deviceId: d.deviceId,
                sessionId: top.sessionId,
                parseState: top.parseState,
                ingestedUtc: top.ingestedUtc,
              });
            }
          } catch {
            // Skip devices whose sessions list fails.
          }
          if (enriched.length >= fleet.matches) break;
        }
        if (!cancelled) setRows(enriched);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [state.fleetResult]);

  if (!state.fleetResult) {
    return (
      <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        Run a query from the KQL bar to populate matches.
      </div>
    );
  }

  return (
    <div style={{ overflow: "auto" }}>
      {loading && (
        <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
          resolving matches…
        </div>
      )}
      {rows.map((r) => {
        const pill = pillFor(r.parseState);
        return (
          <button
            key={r.deviceId + r.sessionId}
            type="button"
            onClick={() => {
              dispatch({ type: "select-device", deviceId: r.deviceId });
              dispatch({ type: "set-middle-mode", mode: "device" });
            }}
            style={{
              all: "unset",
              display: "grid",
              gridTemplateColumns: "1fr 120px 1fr",
              gap: "0.55rem",
              padding: "0.4rem 0.7rem",
              borderBottom: `1px solid ${theme.surfaceAlt}`,
              cursor: "pointer",
              fontFamily: theme.font.mono,
              fontSize: "0.68rem",
              color: theme.text,
            }}
          >
            <span style={{ color: theme.accent, overflow: "hidden", textOverflow: "ellipsis" }}>{r.deviceId}</span>
            <span
              style={{
                padding: "0 5px",
                borderRadius: 2,
                background: theme.pill[pill].bg,
                color: theme.pill[pill].fg,
                fontSize: "0.6rem",
                alignSelf: "center",
                textAlign: "center",
              }}
            >
              {r.parseState}
            </span>
            <span style={{ color: theme.textDim, textAlign: "right" }}>{new Date(r.ingestedUtc).toISOString().slice(11, 16)}Z</span>
          </button>
        );
      })}
    </div>
  );
}
