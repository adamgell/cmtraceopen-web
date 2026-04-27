// Left rail — the list of devices. Mounts in CommandBridge's `rail` region
// and feeds the bridge state's `selectedDeviceId` when the operator clicks
// a row.
//
// Data flow:
//   1. listDevices()                  → page of DeviceSummary
//   2. per-device listSessions(id)    → top session gives us `parseState`
//   3. deriveHealth(parseState, age)  → pill color for the health dot
//
// The N+1 on (2) is intentional for now — spec §4.2 flags it as the
// expected shape until we have >100 devices. If a single device's
// listSessions rejects we swallow the error and the dot falls back to
// neutral gray; one bad device shouldn't sink the whole rail.
//
// Rail expanded vs collapsed state is read from BridgeState. When
// `railExpanded` is true, we also render a search input and a
// `DEVICES · N` header. When false, we render a compact icon strip.
//
// Error handling: if the top-level `listDevices()` call rejects, the rail
// swaps to an inline banner with a retry button. The retry button bumps
// a `reloadNonce` which re-triggers the useEffect's fetch cycle.

import { useCallback, useEffect, useMemo, useState } from "react";
import { listDevices, listSessions } from "../../lib/api-client";
import { useBridgeState } from "../../lib/bridge-state";
import { deriveHealth } from "../../lib/health-dot";
import { theme, type PillState } from "../../lib/theme";
import { DeviceRow, type RailDevice } from "./DeviceRow";
import { SavedViews } from "./SavedViews";

interface FetchState {
  status: "loading" | "ok" | "error";
  devices: RailDevice[];
  error?: string;
}

function formatDelta(lastSeenMs: number | null, nowMs: number): string {
  if (lastSeenMs == null) return "—";
  const diff = nowMs - lastSeenMs;
  const m = Math.floor(diff / 60_000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

export function DeviceRail() {
  const { state, dispatch } = useBridgeState();
  const [fetchState, setFetchState] = useState<FetchState>({ status: "loading", devices: [] });
  const [filter, setFilter] = useState("");
  const [reloadNonce, setReloadNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setFetchState({ status: "loading", devices: [] });
    (async () => {
      try {
        const page = await listDevices();
        // For each device, fetch the most-recent session to know its parse_state.
        // N+1 is acceptable for <100 devices (see spec §4.2 open question).
        const now = Date.now();
        const enriched: RailDevice[] = await Promise.all(
          page.items.map(async (d) => {
            let parseState = "pending";
            try {
              const sessions = await listSessions(d.deviceId);
              const top = sessions.items[0];
              if (top) parseState = top.parseState;
            } catch {
              // Leave pending; the dot will show neutral gray.
            }
            const lastSeenMs = d.lastSeenUtc ? new Date(d.lastSeenUtc).getTime() : null;
            const health: PillState = deriveHealth({ parseState, lastSeenMs }, now);
            return {
              deviceId: d.deviceId,
              lastSeenLabel: formatDelta(lastSeenMs, now),
              health,
            };
          })
        );
        if (!cancelled) setFetchState({ status: "ok", devices: enriched });
      } catch (err) {
        if (!cancelled) {
          setFetchState({
            status: "error",
            devices: [],
            error: err instanceof Error ? err.message : String(err),
          });
        }
      }
    })();
    return () => { cancelled = true; };
  }, [reloadNonce]);

  const visible = useMemo(() => {
    if (!filter.trim()) return fetchState.devices;
    const needle = filter.toLowerCase();
    return fetchState.devices.filter((d) => d.deviceId.toLowerCase().includes(needle));
  }, [fetchState.devices, filter]);

  const onSelect = useCallback(
    (deviceId: string) => dispatch({ type: "select-device", deviceId }),
    [dispatch]
  );

  if (fetchState.status === "error") {
    return (
      <div style={{ padding: "0.7rem", color: theme.pill.failed.fg, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        <div>fleet unreachable</div>
        <div style={{ color: theme.textDim, marginTop: "0.25rem", fontSize: "0.6rem" }}>{fetchState.error}</div>
        <button
          type="button"
          onClick={() => setReloadNonce((n) => n + 1)}
          style={{
            marginTop: "0.5rem",
            padding: "0.25rem 0.6rem",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            color: theme.accent,
            fontFamily: theme.font.mono,
            fontSize: "0.65rem",
            cursor: "pointer",
          }}
        >
          retry
        </button>
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <style>{`@keyframes cmtrace-spin { to { transform: rotate(360deg) } }`}</style>
      {state.railExpanded && (
        <div style={{ padding: "0.55rem 0.7rem", borderBottom: `1px solid ${theme.border}` }}>
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="search devices…"
            style={{
              width: "100%",
              background: theme.surface,
              border: `1px solid ${theme.border}`,
              color: theme.text,
              padding: "0.3rem 0.5rem",
              fontFamily: theme.font.mono,
              fontSize: "0.72rem",
              borderRadius: 3,
            }}
          />
        </div>
      )}
      <div
        style={{
          padding: "0.3rem 0.7rem",
          fontFamily: theme.font.mono,
          fontSize: "0.58rem",
          color: theme.textDim,
          textTransform: "uppercase",
          letterSpacing: "0.1em",
          borderBottom: `1px solid ${theme.border}`,
          display: state.railExpanded ? "block" : "none",
        }}
      >
        DEVICES · {visible.length}
      </div>
      <div style={{ flex: 1, overflow: "auto", padding: state.railExpanded ? 0 : "0.35rem 0" }}>
        {fetchState.status === "loading" && (
          <div style={{ display: "flex", alignItems: "center", gap: "0.4rem", padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
            <span style={{ display: "inline-block", width: 12, height: 12, border: `2px solid ${theme.border}`, borderTopColor: theme.accent, borderRadius: "50%", animation: "cmtrace-spin 0.8s linear infinite" }} />
            loading devices…
          </div>
        )}
        {visible.map((d) => (
          <DeviceRow
            key={d.deviceId}
            device={d}
            expanded={state.railExpanded}
            active={state.selectedDeviceId === d.deviceId}
            onSelect={onSelect}
          />
        ))}
      </div>
      <SavedViews
        expanded={state.railExpanded}
        onRun={(q) => dispatch({ type: "set-fleet-query", query: q })}
      />
    </div>
  );
}
