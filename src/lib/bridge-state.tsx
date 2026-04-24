// Shell-level state for the command-bridge. Plain React context + reducer;
// deliberately no Redux / Zustand / Jotai. Consumers read via useBridgeState()
// which returns { state, dispatch }.
//
// State lives for the lifetime of the shell. Rail expanded/collapsed is
// mirrored to localStorage ("cmtrace.rail-expanded") so it survives reloads;
// everything else is in-memory.

import {
  createContext,
  useContext,
  useReducer,
  useEffect,
  type ReactNode,
  type Dispatch,
} from "react";

export type MiddleMode = "device" | "fleet";

export interface BridgeState {
  railExpanded: boolean;
  selectedDeviceId: string | null;
  selectedSessionId: string | null;
  selectedFileId: string | null;
  middleMode: MiddleMode;
  fleetQuery: string;
  fleetResult: FleetResultSummary | null;
}

export interface FleetResultSummary {
  matches: number;
  devices: number;
  sessions: number;
  files: number;
  groupBy: string;
}

export type BridgeAction =
  | { type: "toggle-rail" }
  | { type: "set-rail"; expanded: boolean }
  | { type: "select-device"; deviceId: string }
  | { type: "select-session"; sessionId: string }
  | { type: "select-file"; sessionId: string; fileId: string }
  | { type: "set-middle-mode"; mode: MiddleMode }
  | { type: "set-fleet-query"; query: string }
  | { type: "set-fleet-result"; result: FleetResultSummary | null };

// Versioned to invalidate preferences saved before the rail defaulted to
// expanded. Old key `cmtrace.rail-expanded` is ignored on purpose — it held
// the collapsed state from the pre-default-expanded UI.
const RAIL_STORAGE_KEY = "cmtrace.rail-expanded.v2";

function initialState(): BridgeState {
  // Default to expanded so full device IDs are visible on first load.
  // Collapsed mode is still reachable via ⌘B and the preference persists.
  let rail = true;
  try {
    const stored = localStorage.getItem(RAIL_STORAGE_KEY);
    if (stored === "1") rail = true;
    else if (stored === "0") rail = false;
  } catch {
    // localStorage may be unavailable (private mode, SSR) — keep default.
  }
  return {
    railExpanded: rail,
    selectedDeviceId: null,
    selectedSessionId: null,
    selectedFileId: null,
    middleMode: "device",
    fleetQuery: "",
    fleetResult: null,
  };
}

function reducer(state: BridgeState, action: BridgeAction): BridgeState {
  switch (action.type) {
    case "toggle-rail":
      return { ...state, railExpanded: !state.railExpanded };
    case "set-rail":
      return { ...state, railExpanded: action.expanded };
    case "select-device":
      // Changing device clears session+file — forces a fresh drill-in.
      return {
        ...state,
        selectedDeviceId: action.deviceId,
        selectedSessionId: null,
        selectedFileId: null,
      };
    case "select-session":
      return { ...state, selectedSessionId: action.sessionId, selectedFileId: null };
    case "select-file":
      return {
        ...state,
        selectedSessionId: action.sessionId,
        selectedFileId: action.fileId,
      };
    case "set-middle-mode":
      return { ...state, middleMode: action.mode };
    case "set-fleet-query":
      return { ...state, fleetQuery: action.query };
    case "set-fleet-result":
      return { ...state, fleetResult: action.result };
    default: {
      // Exhaustiveness check — if a new BridgeAction variant is added and
      // a case is missed, TypeScript flags this line because `_exhaustive`
      // is typed `never`. Protects the state layer across 17 downstream tasks.
      const _exhaustive: never = action;
      return _exhaustive;
    }
  }
}

interface BridgeCtx {
  state: BridgeState;
  dispatch: Dispatch<BridgeAction>;
}

const Ctx = createContext<BridgeCtx | null>(null);

export function BridgeStateProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, undefined, initialState);

  useEffect(() => {
    try {
      localStorage.setItem(RAIL_STORAGE_KEY, state.railExpanded ? "1" : "0");
    } catch {
      // Non-fatal — same reasoning as the read side.
    }
  }, [state.railExpanded]);

  return <Ctx.Provider value={{ state, dispatch }}>{children}</Ctx.Provider>;
}

export function useBridgeState(): BridgeCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useBridgeState must be used inside <BridgeStateProvider>");
  return ctx;
}
