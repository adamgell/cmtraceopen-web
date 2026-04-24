# Viewer command-bridge shell implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the CMTrace Open web viewer as a single "command-bridge" shell: three-pane IDE-style layout with a dense log grid, a global KQL query bar (UI only in v1, executor stubbed), and the DeviceLogViewer aesthetic carried across the whole app.

**Architecture:** One top-level `<CommandBridge>` replaces today's `<ViewerShell>` mode-switching tree. Four stacked regions: KQL bar / device banner / three-pane body (rail + middle + right) / status bar nested in the right pane. State lives in a React context (`useBridgeState`). Legacy components (`ViewerShell`, `DeviceLogViewer`, `FilesPanel`, `Toolbar`, `ApiMode`, `FileSidebar`) are deleted at cutover (Task 18). `LocalMode` and `DiffView` stay functional but get a token-styling pass in Task 19.

**Tech Stack:** React 19, Vite 6, TypeScript 6, vitest 2 + @testing-library/react 16 for tests, `@tanstack/react-virtual` for virtualization (already a dep). No new runtime dependencies. Fluent UI stays for LocalMode/DiffView only; new shell components use the theme tokens directly.

---

## Reference spec

`docs/superpowers/specs/2026-04-24-viewer-command-bridge-design.md`. Pain points addressed: B (search/sort/filter devices), D (signal-rich health dots), E (session tree breathing room), F (cross-device queries via KQL + fleet mode).

## File structure

**New source files (18):**

```
src/lib/theme.ts                         # token module (dark-only)
src/lib/bridge-state.tsx                 # React context + reducer for shell state
src/lib/health-dot.ts                    # parse_state → dot color
src/lib/kql-schema.ts                    # static schema map (tables + fields)
src/lib/kql-lexer.ts                     # tokenizer for query highlighting
src/lib/kql-executor-stub.ts             # canned-response executor
src/lib/keyboard-shortcuts.tsx           # global shortcut registry hook
src/components/shell/CommandBridge.tsx   # top-level shell container
src/components/shell/Banner.tsx          # dotted-header device banner
src/components/shell/KqlBar.tsx          # query bar + autocomplete + result strip
src/components/rail/DeviceRail.tsx       # left rail (collapsed+expanded)
src/components/rail/DeviceRow.tsx        # one device row
src/components/rail/SavedViews.tsx       # pinned KQL queries below devices
src/components/middle/MiddlePane.tsx     # device/fleet tabs
src/components/middle/SessionTree.tsx    # sessions+files tree (device mode)
src/components/middle/FleetList.tsx      # flat match list (fleet mode)
src/components/right/LogViewer.tsx       # right-pane container
src/components/right/EntryGrid.tsx       # virtualized 6-column grid
src/components/right/RowDetail.tsx       # slide-out detail panel
src/components/overlays/LocalOverlay.tsx # local-mode drag-drop overlay
src/components/overlays/HelpOverlay.tsx  # keyboard shortcut help (opens on ?)
```

**Test files (matching):** every module above gets a sibling `<name>.test.{ts,tsx}`.

**Modified:**
- `src/main.tsx` — gate on `?v=next` (Task 3), flip default (Task 18).
- `src/components/LocalMode.tsx` — token adoption (Task 19).
- `src/components/log-view/DiffView.tsx` — token adoption (Task 19).

**Deleted at cutover (Task 18):**
- `src/components/ViewerShell.tsx`, `ApiMode.tsx`, `DeviceLogViewer.tsx`, `FilesPanel.tsx`
- `src/components/layout/Toolbar.tsx`, `FileSidebar.tsx`, `TabStrip.tsx` (if unused post-cutover)
- Potentially `src/lib/workspace-context.tsx` — audit at cutover; bridge-state replaces its role.

## Task list

| # | Task | Build-order step (spec) |
|---|---|---|
| 1 | Theme tokens module | 1 |
| 2 | Bridge state context | 1 |
| 3 | CommandBridge skeleton + gate | 2 |
| 4 | Device banner | 2 |
| 5 | Health-dot utility | 3 |
| 6 | Device rail (collapsed + expanded + search) | 3 |
| 7 | Saved views section in rail | 3 |
| 8 | Middle pane tabs + device-mode session tree | 4 |
| 9 | Log viewer + dense entry grid | 5 |
| 10 | Filter bar + status bar | 5 |
| 11 | Row detail panel | 8 |
| 12 | KQL lexer + schema module | 6 |
| 13 | KQL bar input + autocomplete + actions | 6 |
| 14 | KQL executor stub + result strip | 6 |
| 15 | Middle pane fleet mode | 7 |
| 16 | Keyboard shortcut registry + help overlay | 9 |
| 17 | Local-mode overlay (drag-drop + ⌘O) | 7 |
| 18 | Cutover — delete legacy, flip default | 10 |
| 19 | Token adoption on LocalMode + DiffView | 11 |

---

## Task 1: Theme tokens module

**Files:**
- Create: `src/lib/theme.ts`
- Test: `src/lib/theme.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// src/lib/theme.test.ts
import { describe, it, expect } from "vitest";
import { theme } from "./theme";

describe("theme tokens", () => {
  it("exposes the dark surface palette", () => {
    expect(theme.bg).toBe("#0b0f14");
    expect(theme.surface).toBe("#11161d");
    expect(theme.border).toBe("#1f2a36");
    expect(theme.accent).toBe("#5ee3c5");
  });

  it("provides a pill entry for each parse_state", () => {
    const states = ["ok", "okFallbacks", "partial", "failed", "pending", "stale"];
    for (const s of states) {
      expect(theme.pill[s as keyof typeof theme.pill]).toMatchObject({
        fg: expect.stringMatching(/^#[0-9a-f]{6}$/i),
        bg: expect.stringMatching(/^#[0-9a-f]{6}$/i),
        dot: expect.stringMatching(/^#[0-9a-f]{6}$/i),
      });
    }
  });

  it("exposes a dotted-pattern background string", () => {
    expect(theme.pattern.dots).toContain("radial-gradient");
    expect(theme.pattern.dots).toContain("12px");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/lib/theme.test.ts`
Expected: FAIL with "Cannot find module './theme'".

- [ ] **Step 3: Write the implementation**

```ts
// src/lib/theme.ts
// Dark-only theme tokens for the command-bridge shell.
//
// Single source of truth. Every new shell component reads colors / fonts /
// background patterns from here. Fluent UI's own `tokens.*` is deliberately
// NOT used by shell components — we want to fully own the look. LocalMode
// and DiffView keep Fluent until the Task 19 cleanup pass.

export const theme = {
  bg: "#0b0f14",
  bgDeep: "#070a0e",
  surface: "#11161d",
  surfaceAlt: "#0d1218",
  border: "#1f2a36",
  textPrimary: "#f3f7fb",
  text: "#c7d1dd",
  textDim: "#7da2c3",
  textFainter: "#3d4a5a",
  accent: "#5ee3c5",
  accentBg: "#0e2d22",
  pill: {
    ok:          { fg: "#5ee3c5", bg: "#0e2d22", dot: "#5ee3c5" },
    okFallbacks: { fg: "#f3c37f", bg: "#3d2e12", dot: "#f3c37f" },
    partial:     { fg: "#e08a45", bg: "#3d2516", dot: "#e08a45" },
    failed:      { fg: "#f38c8c", bg: "#3d1414", dot: "#f38c8c" },
    pending:     { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
    stale:       { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
  },
  font: {
    mono: "ui-monospace, Menlo, Consolas, monospace",
    ui: "ui-sans-serif, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  pattern: {
    dots: "radial-gradient(#1c2735 1px, transparent 1px) 0 0 / 12px 12px",
  },
} as const;

export type PillState = keyof typeof theme.pill;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm vitest run src/lib/theme.test.ts`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/theme.ts src/lib/theme.test.ts
git commit -m "feat(shell): add theme tokens for command-bridge redesign"
```

---

## Task 2: Bridge state context

Shell-level state shared across KQL bar, rail, middle pane, right pane. Plain React context + reducer — no extra dependency.

**Files:**
- Create: `src/lib/bridge-state.tsx`
- Test: `src/lib/bridge-state.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// src/lib/bridge-state.test.tsx
import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { BridgeStateProvider, useBridgeState } from "./bridge-state";
import type { ReactNode } from "react";

function wrapper({ children }: { children: ReactNode }) {
  return <BridgeStateProvider>{children}</BridgeStateProvider>;
}

describe("bridge state", () => {
  it("starts with rail collapsed and no selection", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    expect(result.current.state.railExpanded).toBe(false);
    expect(result.current.state.selectedDeviceId).toBeNull();
    expect(result.current.state.middleMode).toBe("device");
  });

  it("toggles the rail", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    act(() => result.current.dispatch({ type: "toggle-rail" }));
    expect(result.current.state.railExpanded).toBe(true);
    act(() => result.current.dispatch({ type: "toggle-rail" }));
    expect(result.current.state.railExpanded).toBe(false);
  });

  it("selects a device and resets session/file", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    act(() => result.current.dispatch({ type: "select-file", sessionId: "s1", fileId: "f1" }));
    act(() => result.current.dispatch({ type: "select-device", deviceId: "GELL-01AA310" }));
    expect(result.current.state.selectedDeviceId).toBe("GELL-01AA310");
    expect(result.current.state.selectedSessionId).toBeNull();
    expect(result.current.state.selectedFileId).toBeNull();
  });

  it("switches middle mode without losing device selection", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    act(() => result.current.dispatch({ type: "select-device", deviceId: "GELL-01AA310" }));
    act(() => result.current.dispatch({ type: "set-middle-mode", mode: "fleet" }));
    expect(result.current.state.middleMode).toBe("fleet");
    expect(result.current.state.selectedDeviceId).toBe("GELL-01AA310");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/lib/bridge-state.test.tsx`
Expected: FAIL with "Cannot find module './bridge-state'".

- [ ] **Step 3: Write the implementation**

```tsx
// src/lib/bridge-state.tsx
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

const RAIL_STORAGE_KEY = "cmtrace.rail-expanded";

function initialState(): BridgeState {
  let rail = false;
  try {
    rail = localStorage.getItem(RAIL_STORAGE_KEY) === "1";
  } catch {
    // localStorage may be unavailable (private mode, SSR) — default collapsed.
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm vitest run src/lib/bridge-state.test.tsx`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/bridge-state.tsx src/lib/bridge-state.test.tsx
git commit -m "feat(shell): add bridge-state context for shell-level state"
```

---

## Task 3: CommandBridge skeleton + gate

Top-level shell with the four-region layout. No real data yet — hardcoded placeholders. Gated behind `?v=next` so the old ViewerShell stays reachable.

**Files:**
- Create: `src/components/shell/CommandBridge.tsx`
- Create: `src/components/shell/CommandBridge.test.tsx`
- Modify: `src/main.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// src/components/shell/CommandBridge.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { CommandBridge } from "./CommandBridge";

describe("CommandBridge skeleton", () => {
  it("renders the four shell regions", () => {
    render(<CommandBridge />);
    expect(screen.getByTestId("kql-bar")).toBeInTheDocument();
    expect(screen.getByTestId("banner")).toBeInTheDocument();
    expect(screen.getByTestId("rail")).toBeInTheDocument();
    expect(screen.getByTestId("middle-pane")).toBeInTheDocument();
    expect(screen.getByTestId("right-pane")).toBeInTheDocument();
    expect(screen.getByTestId("status-bar")).toBeInTheDocument();
  });

  it("defaults the rail width to the collapsed size (56px)", () => {
    render(<CommandBridge />);
    const rail = screen.getByTestId("rail");
    expect(rail.style.width).toBe("56px");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/components/shell/CommandBridge.test.tsx`
Expected: FAIL with "Cannot find module './CommandBridge'".

- [ ] **Step 3: Write the implementation**

```tsx
// src/components/shell/CommandBridge.tsx
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm vitest run src/components/shell/CommandBridge.test.tsx`
Expected: PASS — 2 tests.

- [ ] **Step 5: Gate the new shell behind `?v=next` in main.tsx**

Open `src/main.tsx` and locate the render call (currently renders `<ViewerShell />`). Wrap in a conditional:

```tsx
// src/main.tsx (edit near the existing render)
import { CommandBridge } from "./components/shell/CommandBridge";
// ...existing imports preserved

const useNextShell = new URLSearchParams(window.location.search).get("v") === "next";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {useNextShell ? <CommandBridge /> : <ViewerShell />}
  </React.StrictMode>
);
```

- [ ] **Step 6: Smoke-test the gate**

Run: `pnpm typecheck && pnpm test`
Expected: typecheck clean; all tests pass (existing + 2 new).

- [ ] **Step 7: Commit**

```bash
git add src/components/shell/CommandBridge.tsx src/components/shell/CommandBridge.test.tsx src/main.tsx
git commit -m "feat(shell): add CommandBridge skeleton gated behind ?v=next"
```

---

## Task 4: Device banner

Replaces the `banner placeholder`. Dotted texture, kicker, name, chips, kbd strip. Purely presentational — reads from bridge state.

**Files:**
- Create: `src/components/shell/Banner.tsx`
- Create: `src/components/shell/Banner.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — mount Banner

- [ ] **Step 1: Write the failing test**

```tsx
// src/components/shell/Banner.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Banner } from "./Banner";

describe("Banner", () => {
  it("renders the empty-state kicker when no device is selected", () => {
    render(<Banner device={null} />);
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.queryByText(/LAST SEEN/)).not.toBeInTheDocument();
  });

  it("renders hostname, chips, and the kbd strip when a device is selected", () => {
    render(
      <Banner
        device={{
          deviceId: "GELL-01AA310",
          lastSeenLabel: "11h",
          sessionCount: 44,
          fileCount: 15,
          parseState: "ok-with-fallbacks",
        }}
      />
    );
    expect(screen.getByText("GELL-01AA310")).toBeInTheDocument();
    expect(screen.getByText("LAST SEEN")).toBeInTheDocument();
    expect(screen.getByText("11h")).toBeInTheDocument();
    expect(screen.getByText("SESSIONS")).toBeInTheDocument();
    expect(screen.getByText("44")).toBeInTheDocument();
    expect(screen.getByText(/focus query/)).toBeInTheDocument();
    expect(screen.getByText(/rail/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/components/shell/Banner.test.tsx`
Expected: FAIL with "Cannot find module './Banner'".

- [ ] **Step 3: Write the implementation**

```tsx
// src/components/shell/Banner.tsx
// Dotted-texture banner. Kicker + hostname on the left, monospace chips
// under the hostname, kbd strip pinned to the right. Empty-state renders
// a plain em-dash so the header height stays stable.

import { theme, type PillState } from "../../lib/theme";

export interface BannerDevice {
  deviceId: string;
  lastSeenLabel: string;      // e.g. "11h", "2m", "stale"
  sessionCount: number;
  fileCount: number;
  parseState: string;         // raw string — mapped to pill via mapPillState
}

function mapPillState(raw: string): PillState {
  switch (raw) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    case "pending": return "pending";
    default: return "pending";
  }
}

interface Props {
  device: BannerDevice | null;
}

export function Banner({ device }: Props) {
  const kicker = device ? "DEVICE · LOGS" : "DEVICE";
  return (
    <div
      data-testid="banner"
      style={{
        padding: "0.5rem 0.9rem",
        borderBottom: `1px solid ${theme.border}`,
        background: theme.bg,
        backgroundImage: theme.pattern.dots,
        display: "flex",
        alignItems: "center",
        gap: "0.9rem",
        fontFamily: theme.font.ui,
      }}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: "0.15rem" }}>
        <span style={{ fontSize: "0.6rem", letterSpacing: "0.18em", color: theme.accent, textTransform: "uppercase" }}>
          {kicker}
        </span>
        <span style={{ fontSize: "0.95rem", fontWeight: 700, color: theme.textPrimary, letterSpacing: "-0.01em" }}>
          {device ? device.deviceId : "—"}
        </span>
      </div>
      {device && (
        <div style={{ display: "flex", gap: "0.4rem", alignItems: "center" }}>
          <Chip k="LAST SEEN" v={device.lastSeenLabel} />
          <Chip k="SESSIONS" v={String(device.sessionCount)} />
          <Chip k="FILES" v={String(device.fileCount)} />
          <Chip k="PARSE" v={device.parseState} pill={mapPillState(device.parseState)} />
        </div>
      )}
      <div style={{ marginLeft: "auto", display: "flex", gap: "0.7rem", fontFamily: theme.font.mono, fontSize: "0.6rem", color: theme.textDim }}>
        <Kbd k="⌘/" label="focus query" />
        <Kbd k="⌘B" label="rail" />
        <Kbd k="⌘K" label="jump" />
        <Kbd k="⌘↑↓" label="next file" />
      </div>
    </div>
  );
}

function Chip({ k, v, pill }: { k: string; v: string; pill?: PillState }) {
  const color = pill ? theme.pill[pill].fg : theme.text;
  return (
    <span
      style={{
        padding: "0.15rem 0.45rem",
        background: theme.surface,
        border: `1px solid ${theme.border}`,
        borderRadius: 3,
        fontFamily: theme.font.mono,
        fontSize: "0.64rem",
        whiteSpace: "nowrap",
      }}
    >
      <span style={{ color: theme.textDim }}>{k} </span>
      <span style={{ color }}>{v}</span>
    </span>
  );
}

function Kbd({ k, label }: { k: string; label: string }) {
  return (
    <span>
      <b style={{ color: theme.accent, fontWeight: 600 }}>{k}</b> {label}
    </span>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm vitest run src/components/shell/Banner.test.tsx`
Expected: PASS — 2 tests.

- [ ] **Step 5: Mount Banner in CommandBridge**

Replace the `banner placeholder` block in `src/components/shell/CommandBridge.tsx` with:

```tsx
// At top of file
import { Banner } from "./Banner";

// In BridgeInner, replace the <div data-testid="banner" ...> block:
<Banner device={null} />
```

(Real device data lands in Task 6 once the rail knows the selected device.)

- [ ] **Step 6: Commit**

```bash
git add src/components/shell/Banner.tsx src/components/shell/Banner.test.tsx src/components/shell/CommandBridge.tsx
git commit -m "feat(shell): add device banner with chips + kbd strip"
```

---

## Task 5: Health-dot utility

Pure function + tiny React dot component. Mapped from the device's most-recent `parse_state` + `lastSeenUtc`.

**Files:**
- Create: `src/lib/health-dot.ts`
- Test: `src/lib/health-dot.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// src/lib/health-dot.test.ts
import { describe, it, expect } from "vitest";
import { deriveHealth } from "./health-dot";

function now() { return new Date("2026-04-24T00:00:00Z").getTime(); }

describe("deriveHealth", () => {
  it("returns stale when lastSeen is > 24h old regardless of parse_state", () => {
    expect(deriveHealth({ parseState: "ok", lastSeenMs: now() - 25 * 3600 * 1000 }, now())).toBe("stale");
    expect(deriveHealth({ parseState: "failed", lastSeenMs: now() - 48 * 3600 * 1000 }, now())).toBe("stale");
  });

  it("maps fresh parse states to their own color", () => {
    const fresh = now() - 5 * 60 * 1000;
    expect(deriveHealth({ parseState: "ok", lastSeenMs: fresh }, now())).toBe("ok");
    expect(deriveHealth({ parseState: "ok-with-fallbacks", lastSeenMs: fresh }, now())).toBe("okFallbacks");
    expect(deriveHealth({ parseState: "partial", lastSeenMs: fresh }, now())).toBe("partial");
    expect(deriveHealth({ parseState: "failed", lastSeenMs: fresh }, now())).toBe("failed");
    expect(deriveHealth({ parseState: "pending", lastSeenMs: fresh }, now())).toBe("pending");
  });

  it("returns pending when parseState is unknown but lastSeen is fresh", () => {
    expect(deriveHealth({ parseState: "mystery", lastSeenMs: now() - 60_000 }, now())).toBe("pending");
  });

  it("returns stale when lastSeen is null", () => {
    expect(deriveHealth({ parseState: "ok", lastSeenMs: null }, now())).toBe("stale");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/lib/health-dot.test.ts`
Expected: FAIL with "Cannot find module './health-dot'".

- [ ] **Step 3: Write the implementation**

```ts
// src/lib/health-dot.ts
// Given a device's most-recent parse_state + last_seen_utc, decide which
// health-dot color to render on the left rail.
//
// Rule: a device is `stale` if it hasn't shipped a bundle in > 24h, even
// if the most recent bundle was clean. Freshness dominates state because
// a "green dot · last seen 2 days ago" is a misleading signal for ops.

import type { PillState } from "./theme";

const STALE_MS = 24 * 3600 * 1000;

export interface HealthInput {
  parseState: string;
  lastSeenMs: number | null;
}

export function deriveHealth(input: HealthInput, nowMs: number): PillState {
  if (input.lastSeenMs == null) return "stale";
  if (nowMs - input.lastSeenMs > STALE_MS) return "stale";
  switch (input.parseState) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    case "pending": return "pending";
    default: return "pending";
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm vitest run src/lib/health-dot.test.ts`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/health-dot.ts src/lib/health-dot.test.ts
git commit -m "feat(rail): add health-dot derivation util"
```

---

## Task 6: Device rail (collapsed + expanded + search)

Full left rail — renders collapsed by default (icons only) or expanded (220px, search + list). Fetches via existing `listDevices()` + per-device `listSessions(limit=1)` for health dots.

**Files:**
- Create: `src/components/rail/DeviceRail.tsx`
- Create: `src/components/rail/DeviceRow.tsx`
- Create: `src/components/rail/DeviceRail.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — mount DeviceRail, pass selected device to Banner

- [ ] **Step 1: Write the failing DeviceRow test first**

```tsx
// src/components/rail/DeviceRow.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DeviceRow } from "./DeviceRow";

describe("DeviceRow", () => {
  const device = { deviceId: "GELL-01AA310", lastSeenLabel: "11h", health: "okFallbacks" as const };

  it("renders collapsed: dot + 4-char slug only", () => {
    render(<DeviceRow device={device} expanded={false} active={false} onSelect={() => {}} />);
    expect(screen.getByText("GELL")).toBeInTheDocument();
    expect(screen.queryByText("GELL-01AA310")).not.toBeInTheDocument();
  });

  it("renders expanded: full id + last-seen delta", () => {
    render(<DeviceRow device={device} expanded={true} active={false} onSelect={() => {}} />);
    expect(screen.getByText("GELL-01AA310")).toBeInTheDocument();
    expect(screen.getByText("11h")).toBeInTheDocument();
  });

  it("fires onSelect with the device id on click", () => {
    let captured = "";
    render(<DeviceRow device={device} expanded={false} active={false} onSelect={(id) => (captured = id)} />);
    fireEvent.click(screen.getByRole("button"));
    expect(captured).toBe("GELL-01AA310");
  });
});
```

- [ ] **Step 2: Write the DeviceRow implementation**

```tsx
// src/components/rail/DeviceRow.tsx
import { theme, type PillState } from "../../lib/theme";

export interface RailDevice {
  deviceId: string;
  lastSeenLabel: string;
  health: PillState;
}

interface Props {
  device: RailDevice;
  expanded: boolean;
  active: boolean;
  onSelect: (deviceId: string) => void;
}

export function DeviceRow({ device, expanded, active, onSelect }: Props) {
  const slug = device.deviceId.replace(/[^A-Z0-9]/g, "").slice(0, 4).padEnd(4, "·");
  const dotColor = theme.pill[device.health].dot;
  const bg = active ? theme.accentBg : "transparent";
  const textColor = active ? theme.accent : theme.text;
  const borderLeft = active ? `2px solid ${theme.accent}` : "2px solid transparent";

  if (!expanded) {
    return (
      <button
        type="button"
        onClick={() => onSelect(device.deviceId)}
        title={`${device.deviceId} · ${device.lastSeenLabel}`}
        style={{
          all: "unset",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: "0.2rem",
          width: "44px",
          padding: "0.35rem 0",
          margin: "0 auto",
          background: bg,
          color: textColor,
          borderRadius: 3,
          cursor: "pointer",
          fontFamily: theme.font.mono,
          fontSize: "0.55rem",
        }}
      >
        <span style={{ width: 7, height: 7, borderRadius: "50%", background: dotColor }} />
        <span>{slug}</span>
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => onSelect(device.deviceId)}
      style={{
        all: "unset",
        display: "flex",
        gap: "0.6rem",
        alignItems: "center",
        padding: "0.55rem 0.7rem",
        background: bg,
        color: textColor,
        borderBottom: `1px solid ${theme.surface}`,
        borderLeft,
        cursor: "pointer",
        fontSize: "0.8rem",
      }}
    >
      <span style={{ width: 10, height: 10, borderRadius: "50%", background: dotColor, flexShrink: 0 }} />
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{device.deviceId}</span>
      <span style={{ marginLeft: "auto", fontFamily: theme.font.mono, fontSize: "0.65rem", color: theme.textDim }}>
        {device.lastSeenLabel}
      </span>
    </button>
  );
}
```

- [ ] **Step 3: Run DeviceRow tests**

Run: `pnpm vitest run src/components/rail/DeviceRow.test.tsx`
Expected: PASS — 3 tests.

- [ ] **Step 4: Write DeviceRail test**

```tsx
// src/components/rail/DeviceRail.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";

beforeEach(() => vi.resetModules());

async function loadRail(devices: { deviceId: string; lastSeenUtc: string; sessionCount: number }[]) {
  vi.doMock("../../lib/api-client", () => ({
    listDevices: async () => ({ items: devices, nextCursor: null }),
    listSessions: async () => ({
      items: devices.map((d) => ({
        sessionId: `${d.deviceId}-s1`,
        deviceId: d.deviceId,
        ingestedUtc: d.lastSeenUtc,
        parseState: "ok-with-fallbacks",
        bundleId: "b",
        collectedUtc: null,
        sizeBytes: 0,
      })),
      nextCursor: null,
    }),
  }));
  const { DeviceRail } = await import("./DeviceRail");
  return DeviceRail;
}

describe("DeviceRail", () => {
  it("renders devices returned by the api client", async () => {
    const DeviceRail = await loadRail([
      { deviceId: "GELL-01AA310", lastSeenUtc: new Date(Date.now() - 60_000).toISOString(), sessionCount: 44 },
    ]);
    render(
      <BridgeStateProvider>
        <DeviceRail />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText("GELL")).toBeInTheDocument());
  });

  it("shows a retry banner when the listDevices call rejects", async () => {
    vi.doMock("../../lib/api-client", () => ({
      listDevices: async () => { throw new Error("fleet down"); },
      listSessions: async () => ({ items: [], nextCursor: null }),
    }));
    const { DeviceRail } = await import("./DeviceRail");
    render(
      <BridgeStateProvider>
        <DeviceRail />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/fleet unreachable/i)).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Write the DeviceRail implementation**

```tsx
// src/components/rail/DeviceRail.tsx
import { useCallback, useEffect, useMemo, useState } from "react";
import { listDevices, listSessions } from "../../lib/api-client";
import { useBridgeState } from "../../lib/bridge-state";
import { deriveHealth } from "../../lib/health-dot";
import { theme, type PillState } from "../../lib/theme";
import { DeviceRow, type RailDevice } from "./DeviceRow";

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
          <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
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
    </div>
  );
}
```

- [ ] **Step 6: Mount DeviceRail in CommandBridge and thread the selected device into Banner**

Edit `src/components/shell/CommandBridge.tsx`:

```tsx
// Replace the <div data-testid="rail" ...> block with:
import { DeviceRail } from "../rail/DeviceRail";

// inside BridgeInner, replace the rail div with:
<div data-testid="rail" style={{ width: railWidth, borderRight: `1px solid ${theme.border}`, overflow: "hidden" }}>
  <DeviceRail />
</div>
```

For Banner wiring in v1: still pass `device={null}` — we don't have the rail-selected device's full details yet (session count, file count, etc.). Those come through in Task 8 once the middle pane knows the session totals. Leave a TODO-free comment in CommandBridge:

```tsx
// Banner reflects the rail's selected device when Task 8 threads middle-
// pane totals through bridge state. For now, pass null so the banner stays
// in its empty state.
<Banner device={null} />
```

- [ ] **Step 7: Run tests + typecheck + commit**

Run: `pnpm typecheck && pnpm vitest run src/components/rail`
Expected: PASS — DeviceRow 3 + DeviceRail 2.

```bash
git add src/components/rail src/components/shell/CommandBridge.tsx
git commit -m "feat(rail): add DeviceRail + DeviceRow with health dots"
```

---

## Task 7: Saved views section

Below the device list, a compact "Saved views" section. Reads ★-prefixed queries from `localStorage`. Clicking runs the KQL query in the shell's query bar (wired fully in Task 13 — here we just render static entries + an onClick stub).

**Files:**
- Create: `src/components/rail/SavedViews.tsx`
- Create: `src/components/rail/SavedViews.test.tsx`
- Modify: `src/components/rail/DeviceRail.tsx` — mount SavedViews under the list

- [ ] **Step 1: Write the failing test**

```tsx
// src/components/rail/SavedViews.test.tsx
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SavedViews } from "./SavedViews";

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("SavedViews", () => {
  it("renders nothing when no saved views exist", () => {
    render(<SavedViews expanded={true} onRun={() => {}} />);
    expect(screen.queryByText(/saved views/i)).not.toBeInTheDocument();
  });

  it("renders entries from localStorage", () => {
    localStorage.setItem(
      "cmtrace.saved-views",
      JSON.stringify([{ name: "failed-24h", query: "DeviceLog | where parse_state == \"failed\"" }])
    );
    render(<SavedViews expanded={true} onRun={() => {}} />);
    expect(screen.getByText(/saved views/i)).toBeInTheDocument();
    expect(screen.getByText("★ failed-24h")).toBeInTheDocument();
  });

  it("fires onRun with the stored query when a saved row is clicked", () => {
    localStorage.setItem(
      "cmtrace.saved-views",
      JSON.stringify([{ name: "v1", query: "q1" }])
    );
    let captured = "";
    render(<SavedViews expanded={true} onRun={(q) => (captured = q)} />);
    fireEvent.click(screen.getByText("★ v1"));
    expect(captured).toBe("q1");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm vitest run src/components/rail/SavedViews.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Write the implementation**

```tsx
// src/components/rail/SavedViews.tsx
import { useEffect, useState } from "react";
import { theme } from "../../lib/theme";

export interface SavedView {
  name: string;
  query: string;
}

const STORAGE_KEY = "cmtrace.saved-views";

export function readSavedViews(): SavedView[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((v): v is SavedView =>
      typeof v === "object" && v != null && typeof v.name === "string" && typeof v.query === "string"
    );
  } catch {
    return [];
  }
}

export function writeSavedViews(views: SavedView[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(views));
  } catch {
    // No-op on storage failure.
  }
}

interface Props {
  expanded: boolean;
  onRun: (query: string) => void;
}

export function SavedViews({ expanded, onRun }: Props) {
  const [views, setViews] = useState<SavedView[]>([]);

  useEffect(() => {
    setViews(readSavedViews());
    // Listen for storage events so opening another tab and saving there updates this rail.
    function onStorage(e: StorageEvent) {
      if (e.key === STORAGE_KEY) setViews(readSavedViews());
    }
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  if (views.length === 0 || !expanded) return null;

  return (
    <div>
      <div
        style={{
          padding: "0.3rem 0.7rem",
          fontFamily: theme.font.mono,
          fontSize: "0.58rem",
          color: theme.textDim,
          textTransform: "uppercase",
          letterSpacing: "0.1em",
          borderTop: `1px solid ${theme.border}`,
          borderBottom: `1px solid ${theme.border}`,
          marginTop: "0.5rem",
        }}
      >
        SAVED VIEWS · {views.length}
      </div>
      {views.map((v) => (
        <button
          key={v.name}
          type="button"
          onClick={() => onRun(v.query)}
          title={v.query}
          style={{
            all: "unset",
            display: "block",
            width: "100%",
            padding: "0.4rem 0.7rem",
            color: theme.accent,
            fontFamily: theme.font.mono,
            fontSize: "0.7rem",
            cursor: "pointer",
            borderBottom: `1px solid ${theme.surface}`,
          }}
        >
          ★ {v.name}
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Mount in DeviceRail**

Edit `src/components/rail/DeviceRail.tsx` — at the bottom of the rendered rail (inside the outer `<div>`, after the overflow scroll block):

```tsx
import { SavedViews } from "./SavedViews";

// in the component body, after the </div> that closes the device-list scroll:
<SavedViews expanded={state.railExpanded} onRun={(q) => dispatch({ type: "set-fleet-query", query: q })} />
```

- [ ] **Step 5: Run tests + commit**

```bash
pnpm vitest run src/components/rail/SavedViews.test.tsx
# PASS — 3 tests
git add src/components/rail/SavedViews.tsx src/components/rail/SavedViews.test.tsx src/components/rail/DeviceRail.tsx
git commit -m "feat(rail): add SavedViews section under device list"
```

---

## Task 8: Middle pane tabs + device-mode session tree

Tab toggle at top (DEVICE · n sessions | FLEET · filter). Device mode shows sessions + files tree. Fleet-mode content is stubbed here — real content lands in Task 15.

**Files:**
- Create: `src/components/middle/MiddlePane.tsx`
- Create: `src/components/middle/SessionTree.tsx`
- Create: `src/components/middle/MiddlePane.test.tsx`
- Create: `src/components/middle/SessionTree.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — mount MiddlePane

- [ ] **Step 1: Write the SessionTree test**

```tsx
// src/components/middle/SessionTree.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";

beforeEach(() => vi.resetModules());

async function loadTree(opts: {
  sessions: Array<{ sessionId: string; ingestedUtc: string; parseState: string }>;
  filesBySession: Record<string, Array<{ fileId: string; relativePath: string; entryCount: number }>>;
}) {
  vi.doMock("../../lib/api-client", () => ({
    listSessions: async () => ({
      items: opts.sessions.map((s) => ({
        sessionId: s.sessionId,
        deviceId: "D",
        bundleId: "B",
        collectedUtc: null,
        ingestedUtc: s.ingestedUtc,
        sizeBytes: 0,
        parseState: s.parseState,
      })),
      nextCursor: null,
    }),
    listFiles: async (_sessionId: string) => ({
      items: opts.filesBySession[_sessionId] ?? [],
      nextCursor: null,
    }),
  }));
  const { SessionTree } = await import("./SessionTree");
  return SessionTree;
}

describe("SessionTree", () => {
  it("lists sessions for the selected device", async () => {
    const SessionTree = await loadTree({
      sessions: [
        { sessionId: "s1", ingestedUtc: "2026-04-24T00:28:00Z", parseState: "ok-with-fallbacks" },
        { sessionId: "s2", ingestedUtc: "2026-04-24T00:13:00Z", parseState: "partial" },
      ],
      filesBySession: {},
    });
    render(
      <BridgeStateProvider>
        <SessionTree deviceId="GELL-01AA310" />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/00:28/)).toBeInTheDocument());
    expect(screen.getByText(/00:13/)).toBeInTheDocument();
  });

  it("loads files when a session row is clicked", async () => {
    const SessionTree = await loadTree({
      sessions: [{ sessionId: "s1", ingestedUtc: "2026-04-24T00:28:00Z", parseState: "ok" }],
      filesBySession: { s1: [{ fileId: "f1", relativePath: "logs/ccmexec.log", entryCount: 1300000 }] },
    });
    render(
      <BridgeStateProvider>
        <SessionTree deviceId="GELL-01AA310" />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/00:28/)).toBeInTheDocument());
    fireEvent.click(screen.getByText(/00:28/));
    await waitFor(() => expect(screen.getByText(/ccmexec\.log/)).toBeInTheDocument());
    expect(screen.getByText(/1\.3M/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Write SessionTree**

```tsx
// src/components/middle/SessionTree.tsx
import { useEffect, useState } from "react";
import { listFiles, listSessions } from "../../lib/api-client";
import type { SessionFile, SessionSummary } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme, type PillState } from "../../lib/theme";

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
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

interface Props {
  deviceId: string;
}

export function SessionTree({ deviceId }: Props) {
  const { state, dispatch } = useBridgeState();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filesBySession, setFilesBySession] = useState<Record<string, SessionFile[]>>({});
  const [filter, setFilter] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setSessions([]);
    setExpanded(new Set());
    setFilesBySession({});
    setFilter("");
    (async () => {
      try {
        const page = await listSessions(deviceId);
        if (!cancelled) setSessions(page.items);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [deviceId]);

  async function toggleSession(sessionId: string) {
    const next = new Set(expanded);
    if (next.has(sessionId)) {
      next.delete(sessionId);
      setExpanded(next);
      return;
    }
    next.add(sessionId);
    setExpanded(next);
    if (!filesBySession[sessionId]) {
      try {
        const page = await listFiles(sessionId);
        setFilesBySession((prev) => ({ ...prev, [sessionId]: page.items }));
      } catch {
        // Fall back to empty file list on failure — one-off fetch errors
        // shouldn't kill the rest of the tree.
        setFilesBySession((prev) => ({ ...prev, [sessionId]: [] }));
      }
    }
  }

  if (error) {
    return (
      <div style={{ padding: "0.7rem", color: theme.pill.failed.fg, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        sessions unreachable: {error}
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={{ padding: "0.3rem 0.5rem", borderBottom: `1px solid ${theme.border}` }}>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter files…"
          style={{
            width: "100%",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            color: theme.text,
            padding: "0.25rem 0.4rem",
            fontFamily: theme.font.mono,
            fontSize: "0.68rem",
            borderRadius: 3,
          }}
        />
      </div>
      <div style={{ flex: 1, overflow: "auto" }}>
        {loading && (
          <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
            loading sessions…
          </div>
        )}
        {sessions.map((s) => {
          const isOpen = expanded.has(s.sessionId);
          const chev = isOpen ? "▾" : "▸";
          const ts = new Date(s.ingestedUtc).toISOString().slice(11, 16) + "Z";
          const pill = pillFor(s.parseState);
          const files = filesBySession[s.sessionId] ?? [];
          const visibleFiles = filter.trim()
            ? files.filter((f) => f.relativePath.toLowerCase().includes(filter.toLowerCase()))
            : files;
          return (
            <div key={s.sessionId}>
              <button
                type="button"
                onClick={() => toggleSession(s.sessionId)}
                style={{
                  all: "unset",
                  display: "flex",
                  width: "100%",
                  padding: "0.25rem 0.6rem",
                  gap: "0.4rem",
                  alignItems: "center",
                  fontFamily: theme.font.mono,
                  fontSize: "0.68rem",
                  color: theme.text,
                  borderBottom: `1px solid ${theme.surfaceAlt}`,
                  cursor: "pointer",
                }}
              >
                <span style={{ color: theme.textFainter, fontSize: "0.55rem" }}>{chev}</span>
                <span>{ts}</span>
                <span
                  style={{
                    fontSize: "0.55rem",
                    padding: "0 5px",
                    borderRadius: 2,
                    background: theme.pill[pill].bg,
                    color: theme.pill[pill].fg,
                  }}
                >
                  {s.parseState}
                </span>
                <span style={{ marginLeft: "auto", fontSize: "0.6rem", color: theme.textDim }}>
                  {files.length || ""}
                </span>
              </button>
              {isOpen && visibleFiles.map((f) => {
                const active = state.selectedFileId === f.fileId;
                return (
                  <button
                    key={f.fileId}
                    type="button"
                    onClick={() => dispatch({ type: "select-file", sessionId: s.sessionId, fileId: f.fileId })}
                    style={{
                      all: "unset",
                      display: "flex",
                      width: "100%",
                      padding: "0.18rem 0.6rem 0.18rem 1.25rem",
                      gap: "0.4rem",
                      alignItems: "center",
                      fontFamily: theme.font.mono,
                      fontSize: "0.65rem",
                      color: active ? theme.accent : theme.text,
                      background: active ? theme.accentBg : "transparent",
                      borderLeft: active ? `2px solid ${theme.accent}` : "2px solid transparent",
                      cursor: "pointer",
                    }}
                  >
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {f.relativePath}
                    </span>
                    <span style={{ marginLeft: "auto", fontSize: "0.58rem", color: theme.textDim }}>
                      {formatCount(f.entryCount)}
                    </span>
                  </button>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Run SessionTree tests**

Run: `pnpm vitest run src/components/middle/SessionTree.test.tsx`
Expected: PASS — 2 tests.

- [ ] **Step 4: Write MiddlePane test**

```tsx
// src/components/middle/MiddlePane.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";

vi.mock("./SessionTree", () => ({ SessionTree: ({ deviceId }: { deviceId: string }) => <div data-testid="tree">tree-{deviceId}</div> }));
vi.mock("./FleetList", () => ({ FleetList: () => <div data-testid="fleet">fleet-list</div> }));

import { MiddlePane } from "./MiddlePane";

describe("MiddlePane", () => {
  it("shows empty copy when no device is selected and mode is device", () => {
    render(
      <BridgeStateProvider>
        <MiddlePane />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/pick a device/i)).toBeInTheDocument();
  });

  it("switches to fleet when the fleet tab is clicked", () => {
    render(
      <BridgeStateProvider>
        <MiddlePane />
      </BridgeStateProvider>
    );
    fireEvent.click(screen.getByRole("button", { name: /FLEET/i }));
    expect(screen.getByTestId("fleet")).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Write MiddlePane and a placeholder FleetList**

```tsx
// src/components/middle/FleetList.tsx
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
```

```tsx
// src/components/middle/MiddlePane.tsx
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
```

- [ ] **Step 6: Mount MiddlePane in CommandBridge**

Edit `src/components/shell/CommandBridge.tsx`, replace the `<div data-testid="middle-pane">` block with:

```tsx
import { MiddlePane } from "../middle/MiddlePane";

// ...
<div data-testid="middle-pane" style={{ borderRight: `1px solid ${theme.border}`, overflow: "hidden" }}>
  <MiddlePane />
</div>
```

- [ ] **Step 7: Run tests + typecheck + commit**

Run: `pnpm typecheck && pnpm vitest run src/components/middle`
Expected: PASS — MiddlePane 2 + SessionTree 2.

```bash
git add src/components/middle src/components/shell/CommandBridge.tsx
git commit -m "feat(middle): device/fleet tabs + session tree"
```

---

## Task 9: Log viewer + dense entry grid (right pane)

6-column virtualized grid. Uses `@tanstack/react-virtual` (already a dep). Fetches via `listEntries()`. Maps DTO → LogEntry through the shared `dto-to-entry.ts`.

**Files:**
- Create: `src/components/right/LogViewer.tsx`
- Create: `src/components/right/EntryGrid.tsx`
- Create: `src/components/right/LogViewer.test.tsx`
- Create: `src/components/right/EntryGrid.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — mount LogViewer

- [ ] **Step 1: Write the EntryGrid test**

```tsx
// src/components/right/EntryGrid.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { EntryGrid } from "./EntryGrid";

describe("EntryGrid", () => {
  it("renders a header row with the six column titles", () => {
    render(<EntryGrid entries={[]} />);
    expect(screen.getByText("LINE")).toBeInTheDocument();
    expect(screen.getByText("TIMESTAMP")).toBeInTheDocument();
    expect(screen.getByText("COMPONENT")).toBeInTheDocument();
    expect(screen.getByText("SEV")).toBeInTheDocument();
    expect(screen.getByText("MESSAGE")).toBeInTheDocument();
  });

  it("renders entry messages + component + timestampDisplay", () => {
    render(
      <EntryGrid
        entries={[
          {
            id: 1,
            lineNumber: 42,
            timestamp: 1776872905000,
            timestampDisplay: "2026-04-23 10:28:25",
            severity: "Warning",
            component: "Uploader",
            message: "retry after 5s",
            thread: undefined,
            threadDisplay: undefined,
            sourceFile: undefined,
            format: "Plain",
            filePath: "f",
            timezoneOffset: undefined,
          },
        ]}
      />
    );
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("2026-04-23 10:28:25")).toBeInTheDocument();
    expect(screen.getByText("Uploader")).toBeInTheDocument();
    expect(screen.getByText("WARN")).toBeInTheDocument();
    expect(screen.getByText("retry after 5s")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Write EntryGrid**

```tsx
// src/components/right/EntryGrid.tsx
// Dense 6-column virtualized log grid. Column widths + row padding are
// spec-locked (§6 of 2026-04-24-viewer-command-bridge-design.md).
//
// Virtualization: @tanstack/react-virtual. Target row height is ~18px at
// font-size 0.7rem + line-height 1.28 + padding 0.12rem top/bottom. We
// pass `estimateSize` and let virtual rows measure themselves.

import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { LogEntry } from "../../lib/log-types";
import { theme } from "../../lib/theme";

const COL_TEMPLATE = "22px 52px 156px 130px 56px 1fr";
const COL_GAP = "0.55rem";

function severityGlyph(sev: string): string {
  switch (sev) {
    case "Warning": return "⚠";
    case "Error": return "✖";
    default: return "·";
  }
}

function severityLabel(sev: string): string {
  switch (sev) {
    case "Warning": return "WARN";
    case "Error": return "ERROR";
    default: return "INFO";
  }
}

function severityColor(sev: string): string {
  switch (sev) {
    case "Warning": return theme.pill.okFallbacks.fg;
    case "Error": return theme.pill.failed.fg;
    default: return theme.textDim;
  }
}

function rowBackground(sev: string, zebra: boolean): string {
  if (sev === "Error") return "rgba(243,140,140,.08)";
  if (sev === "Warning") return "rgba(243,195,127,.06)";
  return zebra ? theme.surfaceAlt : "transparent";
}

interface Props {
  entries: LogEntry[];
}

export function EntryGrid({ entries }: Props) {
  const parentRef = useRef<HTMLDivElement | null>(null);
  const virt = useVirtualizer({
    count: entries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 18,
    overscan: 20,
  });

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: COL_TEMPLATE,
          gap: COL_GAP,
          padding: "0.22rem 0.7rem",
          borderBottom: `1px solid ${theme.border}`,
          background: theme.surface,
          color: theme.textDim,
          fontFamily: theme.font.mono,
          fontSize: "0.58rem",
          letterSpacing: "0.08em",
          textTransform: "uppercase",
        }}
      >
        <span />
        <span>LINE</span>
        <span>TIMESTAMP</span>
        <span>COMPONENT</span>
        <span>SEV</span>
        <span>MESSAGE</span>
      </div>
      <div
        ref={parentRef}
        style={{
          flex: 1,
          overflow: "auto",
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
          lineHeight: 1.28,
        }}
      >
        {entries.length === 0 && (
          <div style={{ padding: "0.7rem", color: theme.textDim, fontSize: "0.65rem" }}>
            no entries to render
          </div>
        )}
        <div style={{ height: virt.getTotalSize(), position: "relative" }}>
          {virt.getVirtualItems().map((v) => {
            const e = entries[v.index];
            const glyph = severityGlyph(e.severity);
            const label = severityLabel(e.severity);
            const color = severityColor(e.severity);
            const bg = rowBackground(e.severity, v.index % 2 === 1);
            return (
              <div
                key={e.id}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  transform: `translateY(${v.start}px)`,
                  width: "100%",
                  display: "grid",
                  gridTemplateColumns: COL_TEMPLATE,
                  gap: COL_GAP,
                  padding: "0.12rem 0.7rem",
                  borderBottom: `1px solid ${theme.surfaceAlt}`,
                  color: theme.text,
                  background: bg,
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                }}
              >
                <span style={{ color }}>{glyph}</span>
                <span style={{ color: theme.textFainter, textAlign: "right" }}>{e.lineNumber}</span>
                <span style={{ color: theme.textDim }}>{e.timestampDisplay ?? "—"}</span>
                <span style={{ color: theme.accent, overflow: "hidden", textOverflow: "ellipsis" }}>
                  {e.component ?? ""}
                </span>
                <span style={{ color }}>{label}</span>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{e.message}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Run EntryGrid tests**

Run: `pnpm vitest run src/components/right/EntryGrid.test.tsx`
Expected: PASS — 2 tests.

- [ ] **Step 4: Write LogViewer test**

```tsx
// src/components/right/LogViewer.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";

beforeEach(() => vi.resetModules());

async function loadViewer(entriesPage: any) {
  vi.doMock("../../lib/api-client", () => ({
    listEntries: async () => entriesPage,
  }));
  const { LogViewer } = await import("./LogViewer");
  return LogViewer;
}

describe("LogViewer", () => {
  it("renders an empty-state message when no file is selected", async () => {
    const LogViewer = await loadViewer({ items: [], nextCursor: null });
    render(
      <BridgeStateProvider>
        <LogViewer />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/pick a file/i)).toBeInTheDocument();
  });

  it("fetches and renders entries when a file is selected via bridge state", async () => {
    const LogViewer = await loadViewer({
      items: [
        {
          entryId: 1,
          fileId: "f1",
          lineNumber: 1,
          tsMs: 1776872905000,
          severity: "Info",
          component: "DataCollection",
          thread: null,
          message: "bundle finalized",
          extras: null,
        },
      ],
      nextCursor: null,
    });
    function Seed() {
      // Dispatch a file selection after mount.
      const { dispatch } = require("../../lib/bridge-state").useBridgeState();
      require("react").useEffect(() => {
        dispatch({ type: "select-file", sessionId: "s1", fileId: "f1" });
      }, []);
      return null;
    }
    render(
      <BridgeStateProvider>
        <Seed />
        <LogViewer />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText("bundle finalized")).toBeInTheDocument());
  });
});
```

- [ ] **Step 5: Write LogViewer**

```tsx
// src/components/right/LogViewer.tsx
import { useEffect, useState } from "react";
import { listEntries } from "../../lib/api-client";
import { dtoToEntry } from "../../lib/dto-to-entry";
import type { LogEntry } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { EntryGrid } from "./EntryGrid";

export function LogViewer() {
  const { state } = useBridgeState();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [status, setStatus] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!state.selectedSessionId || !state.selectedFileId) {
      setEntries([]);
      setStatus("idle");
      return;
    }
    let cancelled = false;
    setStatus("loading");
    setError(null);
    (async () => {
      try {
        const page = await listEntries(state.selectedSessionId!, {
          file: state.selectedFileId!,
          limit: 500,
        });
        if (!cancelled) {
          setEntries(page.items.map(dtoToEntry));
          setStatus("ok");
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setStatus("error");
        }
      }
    })();
    return () => { cancelled = true; };
  }, [state.selectedSessionId, state.selectedFileId]);

  if (status === "idle") {
    return (
      <div style={{ padding: "1rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        Pick a file in the middle pane to load entries.
      </div>
    );
  }
  if (status === "error") {
    return (
      <div style={{ padding: "0.7rem", color: theme.pill.failed.fg, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        entries unreachable: {error}
      </div>
    );
  }
  return <EntryGrid entries={entries} />;
}
```

- [ ] **Step 6: Mount LogViewer in CommandBridge**

Edit `src/components/shell/CommandBridge.tsx`, replace the `right-pane content` span inside the right-pane grid:

```tsx
import { LogViewer } from "../right/LogViewer";

// Replace the <div style={{ overflow: "auto", padding: "0.5rem", ... }}> block with:
<div style={{ overflow: "hidden", minHeight: 0 }}>
  <LogViewer />
</div>
```

- [ ] **Step 7: Run tests + typecheck + commit**

```bash
pnpm typecheck && pnpm vitest run src/components/right
# PASS — EntryGrid 2 + LogViewer 2
git add src/components/right src/components/shell/CommandBridge.tsx
git commit -m "feat(right): dense 6-column log grid + LogViewer shell"
```

---

## Task 10: Filter bar + status bar

Filter bar above the grid: severity pills (Info/Warn/Error toggleable), message search, component selector, match counter. Status bar below: rows shown / total, warn/err counts, shortcut hints.

**Files:**
- Create: `src/components/right/FilterBar.tsx`
- Create: `src/components/right/StatusBar.tsx`
- Create: `src/components/right/FilterBar.test.tsx`
- Create: `src/components/right/StatusBar.test.tsx`
- Modify: `src/components/right/LogViewer.tsx` — thread filters + counts through

- [ ] **Step 1: Write FilterBar test**

```tsx
// src/components/right/FilterBar.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterBar } from "./FilterBar";

describe("FilterBar", () => {
  it("toggles severity pills", () => {
    const onChange = vi.fn();
    render(
      <FilterBar
        filters={{ info: true, warn: true, error: true, search: "", component: "" }}
        totals={{ rendered: 500, total: 1_300_000 }}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /info/i }));
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ info: false, warn: true, error: true })
    );
  });

  it("shows the rendered / total counter", () => {
    render(
      <FilterBar
        filters={{ info: true, warn: true, error: true, search: "", component: "" }}
        totals={{ rendered: 500, total: 1_300_000 }}
        onChange={() => {}}
      />
    );
    expect(screen.getByText(/500 \/ 1\.3M/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Write FilterBar**

```tsx
// src/components/right/FilterBar.tsx
import { theme } from "../../lib/theme";

export interface Filters {
  info: boolean;
  warn: boolean;
  error: boolean;
  search: string;
  component: string;
}

export interface Totals {
  rendered: number;
  total: number;
}

function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

interface Props {
  filters: Filters;
  totals: Totals;
  onChange: (next: Filters) => void;
}

export function FilterBar({ filters, totals, onChange }: Props) {
  const pill = (key: "info" | "warn" | "error", label: string) => {
    const on = filters[key];
    const severityBg = key === "error" ? theme.pill.failed.bg : theme.accentBg;
    const severityFg = key === "error" ? theme.pill.failed.fg : theme.accent;
    return (
      <button
        type="button"
        onClick={() => onChange({ ...filters, [key]: !on })}
        style={{
          all: "unset",
          padding: "0.15rem 0.45rem",
          border: `1px solid ${on ? severityFg : theme.border}`,
          borderRadius: 3,
          color: on ? severityFg : theme.textDim,
          background: on ? severityBg : "transparent",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          cursor: "pointer",
        }}
      >
        {label}
      </button>
    );
  };
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.3rem",
        padding: "0.3rem 0.7rem",
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: theme.font.mono,
        fontSize: "0.65rem",
      }}
    >
      {pill("info", "Info")}
      {pill("warn", "Warn")}
      {pill("error", "Error")}
      <input
        value={filters.search}
        onChange={(e) => onChange({ ...filters, search: e.target.value })}
        placeholder="search message…"
        style={{
          flex: 1,
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.text,
          padding: "0.18rem 0.4rem",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          borderRadius: 3,
        }}
      />
      <input
        value={filters.component}
        onChange={(e) => onChange({ ...filters, component: e.target.value })}
        placeholder="Component…"
        style={{
          width: "120px",
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.text,
          padding: "0.18rem 0.4rem",
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          borderRadius: 3,
        }}
      />
      <span style={{ color: theme.textDim, marginLeft: "0.3rem" }}>
        {compact(totals.rendered)} / {compact(totals.total)}
      </span>
    </div>
  );
}
```

- [ ] **Step 3: Run FilterBar tests**

Run: `pnpm vitest run src/components/right/FilterBar.test.tsx`
Expected: PASS — 2 tests.

- [ ] **Step 4: Write StatusBar test**

```tsx
// src/components/right/StatusBar.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { StatusBar } from "./StatusBar";

describe("StatusBar", () => {
  it("renders row counts + severity totals", () => {
    render(<StatusBar rendered={28} limit={500} total={1_300_000} warnCount={2} errCount={2} />);
    expect(screen.getByText(/28 \/ 500/)).toBeInTheDocument();
    expect(screen.getByText(/1\.3M total/)).toBeInTheDocument();
    expect(screen.getByText(/2 warn/)).toBeInTheDocument();
    expect(screen.getByText(/2 err/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Write StatusBar**

```tsx
// src/components/right/StatusBar.tsx
import { theme } from "../../lib/theme";

function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

interface Props {
  rendered: number;
  limit: number;
  total: number;
  warnCount: number;
  errCount: number;
}

export function StatusBar({ rendered, limit, total, warnCount, errCount }: Props) {
  return (
    <div
      data-testid="status-bar"
      style={{
        borderTop: `1px solid ${theme.border}`,
        padding: "0.3rem 0.7rem",
        fontFamily: theme.font.mono,
        fontSize: "0.6rem",
        color: theme.textDim,
        display: "flex",
        gap: "1rem",
      }}
    >
      <span>rows <b style={{ color: theme.text }}>{rendered} / {limit}</b></span>
      <span>· {compact(total)} total</span>
      <span>· <span style={{ color: theme.pill.okFallbacks.fg }}>{warnCount} warn</span> <span style={{ color: theme.pill.failed.fg }}>{errCount} err</span></span>
      <span style={{ marginLeft: "auto" }}>
        <b style={{ color: theme.accent }}>⌘↑↓</b> next file · <b style={{ color: theme.accent }}>J/K</b> row · <b style={{ color: theme.accent }}>/</b> find
      </span>
    </div>
  );
}
```

- [ ] **Step 6: Thread filters + counts through LogViewer**

Rewrite `src/components/right/LogViewer.tsx`:

```tsx
import { useEffect, useMemo, useState } from "react";
import { listEntries } from "../../lib/api-client";
import { dtoToEntry } from "../../lib/dto-to-entry";
import type { LogEntry } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { EntryGrid } from "./EntryGrid";
import { FilterBar, type Filters } from "./FilterBar";
import { StatusBar } from "./StatusBar";

const DEFAULT_FILTERS: Filters = { info: true, warn: true, error: true, search: "", component: "" };

export function LogViewer() {
  const { state } = useBridgeState();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [totalEntriesInFile, setTotalEntriesInFile] = useState(0);
  const [filters, setFilters] = useState<Filters>(DEFAULT_FILTERS);
  const [status, setStatus] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setFilters(DEFAULT_FILTERS);
    if (!state.selectedSessionId || !state.selectedFileId) {
      setEntries([]);
      setStatus("idle");
      setTotalEntriesInFile(0);
      return;
    }
    let cancelled = false;
    setStatus("loading");
    setError(null);
    (async () => {
      try {
        const page = await listEntries(state.selectedSessionId!, {
          file: state.selectedFileId!,
          limit: 500,
        });
        if (!cancelled) {
          const mapped = page.items.map(dtoToEntry);
          setEntries(mapped);
          // total entries in this file is not directly returned — use the
          // session tree's file.entry_count as authoritative; fall back to
          // the rendered length for v1.
          setTotalEntriesInFile(mapped.length);
          setStatus("ok");
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setStatus("error");
        }
      }
    })();
    return () => { cancelled = true; };
  }, [state.selectedSessionId, state.selectedFileId]);

  const visible = useMemo(() => {
    const q = filters.search.toLowerCase();
    const comp = filters.component.toLowerCase();
    return entries.filter((e) => {
      if (e.severity === "Info" && !filters.info) return false;
      if (e.severity === "Warning" && !filters.warn) return false;
      if (e.severity === "Error" && !filters.error) return false;
      if (q && !e.message.toLowerCase().includes(q)) return false;
      if (comp && !(e.component ?? "").toLowerCase().includes(comp)) return false;
      return true;
    });
  }, [entries, filters]);

  const warnCount = visible.filter((e) => e.severity === "Warning").length;
  const errCount = visible.filter((e) => e.severity === "Error").length;

  if (status === "idle") {
    return (
      <div style={{ padding: "1rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        Pick a file in the middle pane to load entries.
      </div>
    );
  }
  if (status === "error") {
    return (
      <div style={{ padding: "0.7rem", color: theme.pill.failed.fg, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        entries unreachable: {error}
      </div>
    );
  }
  return (
    <div style={{ display: "grid", gridTemplateRows: "auto auto 1fr auto", height: "100%", minHeight: 0 }}>
      <FileCrumb />
      <FilterBar filters={filters} totals={{ rendered: visible.length, total: totalEntriesInFile }} onChange={setFilters} />
      <EntryGrid entries={visible} />
      <StatusBar rendered={visible.length} limit={500} total={totalEntriesInFile} warnCount={warnCount} errCount={errCount} />
    </div>
  );
}

function FileCrumb() {
  const { state } = useBridgeState();
  return (
    <div
      style={{
        padding: "0.3rem 0.7rem",
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: theme.font.mono,
        fontSize: "0.66rem",
        color: theme.textDim,
      }}
    >
      <span style={{ color: theme.accent }}>{state.selectedDeviceId ?? "—"}</span>
      {" / "}
      <span>{state.selectedSessionId?.slice(0, 8) ?? "—"}</span>
      {" / "}
      <span style={{ color: theme.accent }}>{state.selectedFileId?.slice(0, 12) ?? "—"}</span>
    </div>
  );
}
```

- [ ] **Step 7: Run tests + commit**

```bash
pnpm vitest run src/components/right
# PASS — EntryGrid 2 + LogViewer 2 + FilterBar 2 + StatusBar 1
git add src/components/right
git commit -m "feat(right): filter bar + status bar wired to log viewer"
```

---

## Task 11: Row detail panel

Slide-out from the right edge when `Enter` is pressed on a focused row. Shows full message + extras JSON. Dismiss on `Esc`.

**Files:**
- Create: `src/components/right/RowDetail.tsx`
- Create: `src/components/right/RowDetail.test.tsx`
- Modify: `src/components/right/EntryGrid.tsx` — keyboard + onOpenRow prop
- Modify: `src/components/right/LogViewer.tsx` — pass selected row to RowDetail

- [ ] **Step 1: Write RowDetail test**

```tsx
// src/components/right/RowDetail.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { RowDetail } from "./RowDetail";
import type { LogEntry } from "../../lib/log-types";

const entry: LogEntry = {
  id: 1,
  lineNumber: 42,
  timestamp: 1776872905000,
  timestampDisplay: "2026-04-23 10:28:25",
  severity: "Info",
  component: "Uploader",
  message: "bundle finalized",
  thread: undefined,
  threadDisplay: undefined,
  sourceFile: undefined,
  format: "Plain",
  filePath: "f",
  timezoneOffset: undefined,
};

describe("RowDetail", () => {
  it("renders null when no entry is provided", () => {
    const { container } = render(<RowDetail entry={null} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("shows the message + metadata when an entry is provided", () => {
    render(<RowDetail entry={entry} onClose={() => {}} />);
    expect(screen.getByText("bundle finalized")).toBeInTheDocument();
    expect(screen.getByText(/42/)).toBeInTheDocument();
    expect(screen.getByText(/Uploader/)).toBeInTheDocument();
  });

  it("calls onClose when the close button is clicked", () => {
    const onClose = vi.fn();
    render(<RowDetail entry={entry} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Write RowDetail**

```tsx
// src/components/right/RowDetail.tsx
import { useEffect } from "react";
import type { LogEntry } from "../../lib/log-types";
import { theme } from "../../lib/theme";

interface Props {
  entry: LogEntry | null;
  onClose: () => void;
}

export function RowDetail({ entry, onClose }: Props) {
  useEffect(() => {
    if (!entry) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [entry, onClose]);

  if (!entry) return null;

  return (
    <aside
      style={{
        position: "absolute",
        top: 0,
        right: 0,
        bottom: 0,
        width: "420px",
        background: theme.bgDeep,
        borderLeft: `1px solid ${theme.border}`,
        padding: "0.9rem 1rem",
        overflow: "auto",
        fontFamily: theme.font.mono,
        fontSize: "0.72rem",
        color: theme.text,
      }}
    >
      <header style={{ display: "flex", alignItems: "center", marginBottom: "0.7rem" }}>
        <span style={{ color: theme.accent, fontSize: "0.6rem", letterSpacing: "0.12em", textTransform: "uppercase" }}>
          Row detail
        </span>
        <button
          type="button"
          onClick={onClose}
          aria-label="close"
          style={{
            all: "unset",
            marginLeft: "auto",
            padding: "0.15rem 0.45rem",
            border: `1px solid ${theme.border}`,
            borderRadius: 3,
            color: theme.textDim,
            cursor: "pointer",
            fontSize: "0.65rem",
          }}
        >
          Esc · close
        </button>
      </header>
      <dl style={{ display: "grid", gridTemplateColumns: "100px 1fr", gap: "0.25rem 0.7rem", margin: 0 }}>
        <dt style={{ color: theme.textDim }}>line</dt><dd style={{ margin: 0 }}>{entry.lineNumber}</dd>
        <dt style={{ color: theme.textDim }}>timestamp</dt><dd style={{ margin: 0 }}>{entry.timestampDisplay ?? "—"}</dd>
        <dt style={{ color: theme.textDim }}>severity</dt><dd style={{ margin: 0 }}>{entry.severity}</dd>
        <dt style={{ color: theme.textDim }}>component</dt><dd style={{ margin: 0 }}>{entry.component ?? "—"}</dd>
        <dt style={{ color: theme.textDim }}>thread</dt><dd style={{ margin: 0 }}>{entry.threadDisplay ?? "—"}</dd>
      </dl>
      <section style={{ marginTop: "0.9rem" }}>
        <div style={{ color: theme.textDim, fontSize: "0.6rem", letterSpacing: "0.1em", textTransform: "uppercase" }}>Message</div>
        <pre style={{ whiteSpace: "pre-wrap", margin: "0.3rem 0 0", color: theme.textPrimary }}>{entry.message}</pre>
      </section>
    </aside>
  );
}
```

- [ ] **Step 3: Wire Enter-to-open in EntryGrid + add onOpenRow prop**

Edit `src/components/right/EntryGrid.tsx`. Change the signature + handler:

```tsx
interface Props {
  entries: LogEntry[];
  onOpenRow?: (entry: LogEntry) => void;
}

export function EntryGrid({ entries, onOpenRow }: Props) {
  // ...existing code...
  // Inside the row rendering, change the row <div> into a keyboard-focusable
  // button-like element:
  //   role="button" tabIndex={0}
  //   onKeyDown={(e) => { if (e.key === "Enter") onOpenRow?.(entries[v.index]); }}
  //   onClick={() => onOpenRow?.(entries[v.index])}
  //   style={{ ...existing, cursor: onOpenRow ? "pointer" : "default" }}
  // (leave all other styles as they were)
}
```

Concrete patch within the row map:

```tsx
<div
  key={e.id}
  role="button"
  tabIndex={0}
  onClick={() => onOpenRow?.(e)}
  onKeyDown={(ev) => { if (ev.key === "Enter") onOpenRow?.(e); }}
  style={{
    // ...existing styles unchanged...
    cursor: onOpenRow ? "pointer" : "default",
  }}
>
```

- [ ] **Step 4: Wire RowDetail in LogViewer**

Edit `src/components/right/LogViewer.tsx`:

```tsx
import { RowDetail } from "./RowDetail";
// ...inside the component state:
const [detailEntry, setDetailEntry] = useState<LogEntry | null>(null);

// Wrap the grid in a relative container + pass onOpenRow + render RowDetail:
return (
  <div style={{ position: "relative", display: "grid", gridTemplateRows: "auto auto 1fr auto", height: "100%", minHeight: 0 }}>
    <FileCrumb />
    <FilterBar filters={filters} totals={{ rendered: visible.length, total: totalEntriesInFile }} onChange={setFilters} />
    <EntryGrid entries={visible} onOpenRow={setDetailEntry} />
    <StatusBar rendered={visible.length} limit={500} total={totalEntriesInFile} warnCount={warnCount} errCount={errCount} />
    <RowDetail entry={detailEntry} onClose={() => setDetailEntry(null)} />
  </div>
);
```

- [ ] **Step 5: Run tests + commit**

```bash
pnpm vitest run src/components/right
# RowDetail 3 + existing all pass
git add src/components/right
git commit -m "feat(right): Enter-to-open row detail panel"
```

---

## Task 12: KQL lexer + schema module

Pure functions. Tokenize an input string into typed tokens for highlighting; static schema map for autocomplete.

**Files:**
- Create: `src/lib/kql-schema.ts`
- Create: `src/lib/kql-lexer.ts`
- Create: `src/lib/kql-schema.test.ts`
- Create: `src/lib/kql-lexer.test.ts`

- [ ] **Step 1: Write the schema test**

```ts
// src/lib/kql-schema.test.ts
import { describe, it, expect } from "vitest";
import { kqlSchema, tablesList, fieldsFor } from "./kql-schema";

describe("kql-schema", () => {
  it("exposes the three tables", () => {
    expect(tablesList()).toEqual(["DeviceLog", "File", "Entry"]);
  });

  it("fieldsFor returns the declared fields for each table", () => {
    expect(fieldsFor("DeviceLog").map((f) => f.name)).toContain("parse_state");
    expect(fieldsFor("File").map((f) => f.name)).toContain("relative_path");
    expect(fieldsFor("Entry").map((f) => f.name)).toContain("ts_ms");
  });

  it("fieldsFor returns empty for unknown tables", () => {
    expect(fieldsFor("Unknown")).toEqual([]);
  });

  it("every field has a type and at least one example", () => {
    for (const t of tablesList()) {
      for (const f of kqlSchema[t]) {
        expect(f.type.length).toBeGreaterThan(0);
        expect(f.examples.length).toBeGreaterThan(0);
      }
    }
  });
});
```

- [ ] **Step 2: Write the schema module**

```ts
// src/lib/kql-schema.ts
// Static schema for the stubbed KQL executor. Autocomplete resolves field
// names and example values from here. Mirrors the api-server's SQLite schema
// exposed through /v1/devices, /v1/sessions, /v1/files, /v1/entries.

export interface KqlField {
  name: string;
  type: "string" | "long" | "datetime";
  examples: string[];
}

export type KqlTable = "DeviceLog" | "File" | "Entry";

export const kqlSchema: Record<KqlTable, KqlField[]> = {
  DeviceLog: [
    { name: "device_id", type: "string", examples: ['"GELL-01AA310"', '"GELL-E9C0C757"'] },
    { name: "parse_state", type: "string", examples: ['"ok"', '"ok-with-fallbacks"', '"partial"', '"failed"', '"pending"'] },
    { name: "ingested_utc", type: "datetime", examples: ["ago(24h)", "ago(7d)"] },
    { name: "collected_utc", type: "datetime", examples: ["ago(24h)"] },
    { name: "size_bytes", type: "long", examples: ["1024", "1000000"] },
  ],
  File: [
    { name: "session_id", type: "string", examples: ['"019dba89..."'] },
    { name: "relative_path", type: "string", examples: ['"logs/ccmexec.log"', '"agent/agent-2026-04-24.log"'] },
    { name: "parser_kind", type: "string", examples: ['"Ccm"', '"TracingJson"', '"IisW3c"'] },
    { name: "entry_count", type: "long", examples: ["0", "1000", "1000000"] },
    { name: "parse_error_count", type: "long", examples: ["0", "94"] },
  ],
  Entry: [
    { name: "file_id", type: "string", examples: ['"019dba89..."'] },
    { name: "line_number", type: "long", examples: ["1", "42"] },
    { name: "ts_ms", type: "long", examples: ["1776872905000"] },
    { name: "severity", type: "string", examples: ['"Info"', '"Warning"', '"Error"'] },
    { name: "component", type: "string", examples: ['"Uploader"', '"DataCollection"'] },
    { name: "message", type: "string", examples: ['"retry after 5s"'] },
  ],
};

export function tablesList(): KqlTable[] {
  return ["DeviceLog", "File", "Entry"];
}

export function fieldsFor(table: string): KqlField[] {
  return kqlSchema[table as KqlTable] ?? [];
}

export const KQL_KEYWORDS = [
  "where", "summarize", "project", "extend", "join", "take", "count", "order", "by", "asc", "desc", "and", "or", "not", "in", "between",
] as const;

export const KQL_FUNCTIONS = [
  "ago", "now", "count", "countif", "sum", "avg", "min", "max", "dcount", "startofday", "endofday",
] as const;

export const KQL_OPERATORS = [
  "==", "!=", ">=", "<=", ">", "<", "has", "contains", "startswith", "endswith",
] as const;
```

- [ ] **Step 3: Run schema tests**

Run: `pnpm vitest run src/lib/kql-schema.test.ts`
Expected: PASS — 4 tests.

- [ ] **Step 4: Write the lexer test**

```ts
// src/lib/kql-lexer.test.ts
import { describe, it, expect } from "vitest";
import { tokenize } from "./kql-lexer";

describe("tokenize", () => {
  it("classifies table, pipe, keyword, field, operator, and string", () => {
    const tokens = tokenize('DeviceLog | where parse_state == "failed"');
    const classes = tokens.map((t) => t.kind);
    expect(classes).toEqual([
      "table", "whitespace",
      "pipe", "whitespace",
      "keyword", "whitespace",
      "field", "whitespace",
      "operator", "whitespace",
      "string",
    ]);
  });

  it("classifies function calls with numeric duration args", () => {
    const tokens = tokenize("ingested_utc > ago(24h)");
    const fn = tokens.find((t) => t.kind === "function");
    const num = tokens.find((t) => t.kind === "number");
    expect(fn?.text).toBe("ago");
    expect(num?.text).toBe("24h");
  });

  it("falls back to ident for unknown identifiers", () => {
    const tokens = tokenize("unknown_thing");
    expect(tokens[0].kind).toBe("ident");
  });

  it("preserves source spans", () => {
    const tokens = tokenize('DeviceLog | where x == 1');
    for (const t of tokens) {
      expect(t.end).toBeGreaterThanOrEqual(t.start);
    }
    // The sum of spans should equal the original length.
    expect(tokens[tokens.length - 1].end).toBe('DeviceLog | where x == 1'.length);
  });
});
```

- [ ] **Step 5: Write the lexer**

```ts
// src/lib/kql-lexer.ts
// Tiny lexer for the KQL subset shown by the query bar. Not a parser — it
// emits a flat token stream that the syntax-highlight renderer can colourise.
// Unknown identifiers fall through to `ident` so the UI can still show them
// (the stubbed executor doesn't enforce schema correctness yet).

import { fieldsFor, tablesList, KQL_KEYWORDS, KQL_FUNCTIONS, KQL_OPERATORS } from "./kql-schema";

export type TokenKind =
  | "table"
  | "pipe"
  | "keyword"
  | "function"
  | "operator"
  | "field"
  | "ident"
  | "string"
  | "number"
  | "whitespace";

export interface Token {
  kind: TokenKind;
  text: string;
  start: number;
  end: number;
}

const FIELD_NAMES = new Set<string>(
  tablesList().flatMap((t) => fieldsFor(t).map((f) => f.name))
);
const TABLES = new Set<string>(tablesList());
const KEYWORDS = new Set<string>(KQL_KEYWORDS);
const FUNCTIONS = new Set<string>(KQL_FUNCTIONS);
const OPERATORS = [...KQL_OPERATORS].sort((a, b) => b.length - a.length);

function isIdentStart(c: string): boolean {
  return /[A-Za-z_]/.test(c);
}
function isIdentPart(c: string): boolean {
  return /[A-Za-z0-9_]/.test(c);
}

function readWhile(input: string, i: number, pred: (c: string) => boolean): number {
  while (i < input.length && pred(input[i])) i++;
  return i;
}

export function tokenize(input: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < input.length) {
    const c = input[i];
    const start = i;

    if (/\s/.test(c)) {
      i = readWhile(input, i, (ch) => /\s/.test(ch));
      tokens.push({ kind: "whitespace", text: input.slice(start, i), start, end: i });
      continue;
    }

    if (c === "|") {
      i++;
      tokens.push({ kind: "pipe", text: "|", start, end: i });
      continue;
    }

    if (c === '"') {
      i++;
      while (i < input.length && input[i] !== '"') i++;
      if (i < input.length) i++; // consume closing quote
      tokens.push({ kind: "string", text: input.slice(start, i), start, end: i });
      continue;
    }

    if (/[0-9]/.test(c)) {
      i = readWhile(input, i, (ch) => /[0-9a-zA-Z]/.test(ch));
      tokens.push({ kind: "number", text: input.slice(start, i), start, end: i });
      continue;
    }

    // Operator match (longest first).
    let matchedOp = "";
    for (const op of OPERATORS) {
      if (input.startsWith(op, i)) {
        matchedOp = op;
        break;
      }
    }
    if (matchedOp) {
      i += matchedOp.length;
      tokens.push({ kind: "operator", text: matchedOp, start, end: i });
      continue;
    }

    if (isIdentStart(c)) {
      i = readWhile(input, i, isIdentPart);
      const text = input.slice(start, i);
      const lower = text.toLowerCase();
      let kind: TokenKind = "ident";
      if (TABLES.has(text)) kind = "table";
      else if (KEYWORDS.has(lower)) kind = "keyword";
      else if (FUNCTIONS.has(lower)) kind = "function";
      else if (FIELD_NAMES.has(text)) kind = "field";
      tokens.push({ kind, text, start, end: i });
      continue;
    }

    // Unrecognised character — classify as ident so the highlighter keeps rendering.
    i++;
    tokens.push({ kind: "ident", text: input.slice(start, i), start, end: i });
  }
  return tokens;
}
```

- [ ] **Step 6: Run lexer tests**

Run: `pnpm vitest run src/lib/kql-lexer.test.ts`
Expected: PASS — 4 tests.

- [ ] **Step 7: Commit**

```bash
git add src/lib/kql-schema.ts src/lib/kql-schema.test.ts src/lib/kql-lexer.ts src/lib/kql-lexer.test.ts
git commit -m "feat(kql): static schema + token lexer"
```

---

## Task 13: KQL bar input + autocomplete + actions

Input with syntax highlighting, Recent/Saved dropdown, Run/Explain/Save buttons. Runs via the stub (Task 14) — here the Run handler calls a prop.

**Files:**
- Create: `src/components/shell/KqlBar.tsx`
- Create: `src/components/shell/KqlBar.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — mount KqlBar

- [ ] **Step 1: Write the test**

```tsx
// src/components/shell/KqlBar.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";
import { KqlBar } from "./KqlBar";

beforeEach(() => {
  localStorage.clear();
});

describe("KqlBar", () => {
  it("renders a monospace input with the RUN button", () => {
    render(
      <BridgeStateProvider>
        <KqlBar onRun={() => {}} />
      </BridgeStateProvider>
    );
    expect(screen.getByRole("textbox")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /run/i })).toBeInTheDocument();
  });

  it("fires onRun with the current query when the run button is clicked", () => {
    const onRun = vi.fn();
    render(
      <BridgeStateProvider>
        <KqlBar onRun={onRun} />
      </BridgeStateProvider>
    );
    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'DeviceLog | where parse_state == "failed"' } });
    fireEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(onRun).toHaveBeenCalledWith('DeviceLog | where parse_state == "failed"');
  });

  it("persists the query as a recent entry on run", () => {
    render(
      <BridgeStateProvider>
        <KqlBar onRun={() => {}} />
      </BridgeStateProvider>
    );
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "DeviceLog | where x == 1" } });
    fireEvent.click(screen.getByRole("button", { name: /run/i }));
    const recent = JSON.parse(localStorage.getItem("cmtrace.recent-queries") ?? "[]");
    expect(recent).toContain("DeviceLog | where x == 1");
  });
});
```

- [ ] **Step 2: Write the implementation**

```tsx
// src/components/shell/KqlBar.tsx
import { useMemo, useState, type KeyboardEvent } from "react";
import { useBridgeState } from "../../lib/bridge-state";
import { tokenize, type Token } from "../../lib/kql-lexer";
import { theme } from "../../lib/theme";
import { readSavedViews, writeSavedViews } from "../rail/SavedViews";

const RECENT_KEY = "cmtrace.recent-queries";
const MAX_RECENT = 10;

function readRecent(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((v) => typeof v === "string") : [];
  } catch { return []; }
}

function writeRecent(queries: string[]) {
  try { localStorage.setItem(RECENT_KEY, JSON.stringify(queries.slice(0, MAX_RECENT))); } catch { /* noop */ }
}

const TOKEN_COLORS: Record<Token["kind"], string> = {
  table: theme.pill.okFallbacks.fg,   // amber
  pipe: theme.accent,
  keyword: "#9a7ef8",                  // purple
  field: theme.textDim,
  operator: theme.accent,
  string: theme.pill.partial.fg,       // orange
  function: theme.accent,
  number: theme.pill.okFallbacks.fg,   // amber
  ident: theme.text,
  whitespace: theme.text,
};

interface Props {
  onRun: (query: string) => void;
}

export function KqlBar({ onRun }: Props) {
  const { state } = useBridgeState();
  const [query, setQuery] = useState(state.fleetQuery);
  const [focused, setFocused] = useState(false);
  const tokens = useMemo(() => tokenize(query), [query]);

  function runNow() {
    if (!query.trim()) return;
    const recent = readRecent();
    writeRecent([query, ...recent.filter((q) => q !== query)]);
    onRun(query);
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") runNow();
    if (e.key === "Escape") (e.target as HTMLInputElement).blur();
  }

  function saveCurrent() {
    if (!query.trim()) return;
    const name = window.prompt("Name this view", query.slice(0, 40)) ?? "";
    if (!name.trim()) return;
    const existing = readSavedViews();
    writeSavedViews([{ name: name.trim(), query }, ...existing.filter((v) => v.name !== name.trim())]);
  }

  return (
    <div style={{ background: theme.bgDeep, borderBottom: `1px solid ${theme.border}`, padding: "0.55rem 0.75rem", display: "flex", gap: "0.6rem", alignItems: "center" }}>
      <span style={{ color: theme.accent, fontFamily: theme.font.mono, fontSize: "0.78rem", letterSpacing: "0.08em" }}>›_</span>
      <div style={{ flex: 1, position: "relative" }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setTimeout(() => setFocused(false), 150)}
          onKeyDown={onKey}
          placeholder='DeviceLog | where parse_state == "failed" | where ingested_utc > ago(24h)'
          style={{
            width: "100%",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            borderRadius: 4,
            padding: "0.4rem 0.6rem",
            fontFamily: theme.font.mono,
            fontSize: "0.78rem",
            color: theme.textPrimary,
            caretColor: theme.accent,
          }}
        />
        <HighlightOverlay tokens={tokens} />
        {focused && <Dropdown query={query} onPick={(q) => { setQuery(q); onRun(q); }} />}
      </div>
      <button
        type="button"
        onClick={runNow}
        style={{
          background: theme.accentBg,
          border: `1px solid ${theme.accent}`,
          color: theme.accent,
          padding: "0.35rem 0.8rem",
          borderRadius: 4,
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
          letterSpacing: "0.06em",
          cursor: "pointer",
        }}
      >
        RUN · ⏎
      </button>
      <button
        type="button"
        onClick={saveCurrent}
        style={{
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.textDim,
          padding: "0.35rem 0.6rem",
          borderRadius: 4,
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          cursor: "pointer",
        }}
      >
        ★ SAVE
      </button>
    </div>
  );
}

function HighlightOverlay({ tokens }: { tokens: Token[] }) {
  // We render a non-interactive overlay on top of the input for visual-only
  // coloring. Because the input is a single-line text input and uses a
  // monospace font identical to this overlay, the overlay aligns character
  // by character. pointer-events:none so clicks still reach the input.
  return (
    <div
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        padding: "0.4rem 0.6rem",
        fontFamily: theme.font.mono,
        fontSize: "0.78rem",
        pointerEvents: "none",
        whiteSpace: "pre",
        color: "transparent",
      }}
    >
      {tokens.map((t, i) => (
        <span key={i} style={{ color: TOKEN_COLORS[t.kind] }}>{t.text}</span>
      ))}
    </div>
  );
}

function Dropdown({ query, onPick }: { query: string; onPick: (q: string) => void }) {
  const recent = readRecent();
  const saved = readSavedViews();
  if (recent.length === 0 && saved.length === 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        top: "100%",
        background: theme.bg,
        border: `1px solid ${theme.border}`,
        borderTop: "none",
        borderRadius: "0 0 4px 4px",
        padding: "0.3rem 0",
        fontFamily: theme.font.mono,
        fontSize: "0.68rem",
        zIndex: 10,
      }}
    >
      {recent.length > 0 && (
        <>
          <div style={{ padding: "0.2rem 0.75rem", color: theme.textDim, fontSize: "0.55rem", letterSpacing: "0.1em", textTransform: "uppercase" }}>
            Recent
          </div>
          {recent.slice(0, 5).map((q) => (
            <button
              key={q}
              type="button"
              onMouseDown={(e) => { e.preventDefault(); onPick(q); }}
              style={{ all: "unset", display: "block", width: "100%", padding: "0.25rem 0.75rem", color: theme.text, cursor: "pointer" }}
            >
              {q}
            </button>
          ))}
        </>
      )}
      {saved.length > 0 && (
        <>
          <div style={{ padding: "0.2rem 0.75rem", color: theme.textDim, fontSize: "0.55rem", letterSpacing: "0.1em", textTransform: "uppercase", marginTop: "0.3rem" }}>
            Saved views
          </div>
          {saved.slice(0, 5).map((v) => (
            <button
              key={v.name}
              type="button"
              onMouseDown={(e) => { e.preventDefault(); onPick(v.query); }}
              style={{ all: "unset", display: "block", width: "100%", padding: "0.25rem 0.75rem", color: theme.accent, cursor: "pointer" }}
            >
              ★ {v.name}
            </button>
          ))}
        </>
      )}
    </div>
  );
}
```

- [ ] **Step 3: Mount in CommandBridge**

Edit `src/components/shell/CommandBridge.tsx`. Replace the `<div data-testid="kql-bar">` placeholder block:

```tsx
import { KqlBar } from "./KqlBar";

// Inside BridgeInner:
<KqlBar onRun={(q) => {/* wired in Task 14 */ console.log("run", q); }} />
```

- [ ] **Step 4: Run tests + commit**

```bash
pnpm typecheck && pnpm vitest run src/components/shell
# PASS — KqlBar 3 + Banner 2 + CommandBridge 2
git add src/components/shell/KqlBar.tsx src/components/shell/KqlBar.test.tsx src/components/shell/CommandBridge.tsx
git commit -m "feat(kql): query bar with tokens + autocomplete + save"
```

---

## Task 14: KQL executor stub + result strip

Stubbed executor that returns a plausible summary based on the query string. Result strip renders under the bar when a run completes.

**Files:**
- Create: `src/lib/kql-executor-stub.ts`
- Create: `src/lib/kql-executor-stub.test.ts`
- Create: `src/components/shell/ResultStrip.tsx`
- Modify: `src/components/shell/KqlBar.tsx` — no change
- Modify: `src/components/shell/CommandBridge.tsx` — wire onRun through the executor + render ResultStrip

- [ ] **Step 1: Write executor test**

```ts
// src/lib/kql-executor-stub.test.ts
import { describe, it, expect } from "vitest";
import { runKqlStub } from "./kql-executor-stub";

describe("runKqlStub", () => {
  it("returns a plausible shape for a DeviceLog query", () => {
    const res = runKqlStub('DeviceLog | where parse_state == "failed"');
    expect(res.matches).toBeGreaterThanOrEqual(0);
    expect(res.devices).toBeGreaterThanOrEqual(0);
    expect(res.sessions).toBeGreaterThanOrEqual(0);
    expect(res.files).toBeGreaterThanOrEqual(0);
    expect(typeof res.groupBy).toBe("string");
  });

  it("infers groupBy = 'device' when the pipeline ends at DeviceLog", () => {
    const res = runKqlStub("DeviceLog | where parse_state == \"failed\"");
    expect(res.groupBy).toBe("device");
  });

  it("infers groupBy = 'file' when the query targets File", () => {
    const res = runKqlStub("File | where relative_path has \"ccmexec\"");
    expect(res.groupBy).toBe("file");
  });

  it("returns zero matches for an empty query", () => {
    const res = runKqlStub("");
    expect(res.matches).toBe(0);
  });
});
```

- [ ] **Step 2: Write the executor stub**

```ts
// src/lib/kql-executor-stub.ts
// Stubbed executor for the KQL bar. Returns a canned summary shape so the
// UI can render a plausible result strip without a real query compiler.
// Inputs the rough table target from the first token (if present).
//
// TODO(real-executor): replace with a real compiler — see design spec
// open question "KQL executor boundary".

import type { FleetResultSummary } from "./bridge-state";
import { tokenize } from "./kql-lexer";

function pseudoNumber(query: string, mod: number): number {
  // Deterministic but looks random so operators don't see perfectly
  // repeated numbers across different queries.
  let h = 0;
  for (let i = 0; i < query.length; i++) h = (h * 31 + query.charCodeAt(i)) | 0;
  return Math.abs(h) % mod;
}

export function runKqlStub(query: string): FleetResultSummary {
  const trimmed = query.trim();
  if (!trimmed) {
    return { matches: 0, devices: 0, sessions: 0, files: 0, groupBy: "device" };
  }
  const tokens = tokenize(trimmed);
  const firstTable = tokens.find((t) => t.kind === "table")?.text ?? "DeviceLog";
  const groupBy =
    firstTable === "Entry" ? "entry" :
    firstTable === "File" ? "file" :
    "device";
  const matches = 10 + pseudoNumber(trimmed, 90);
  return {
    matches,
    devices: 1 + pseudoNumber(trimmed, 12),
    sessions: 1 + pseudoNumber(trimmed + "s", 50),
    files: 1 + pseudoNumber(trimmed + "f", 25),
    groupBy,
  };
}
```

- [ ] **Step 3: Run executor tests**

Run: `pnpm vitest run src/lib/kql-executor-stub.test.ts`
Expected: PASS — 4 tests.

- [ ] **Step 4: Write ResultStrip**

```tsx
// src/components/shell/ResultStrip.tsx
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
```

- [ ] **Step 5: Wire Run in CommandBridge**

Edit `src/components/shell/CommandBridge.tsx`:

```tsx
import { runKqlStub } from "../../lib/kql-executor-stub";
import { ResultStrip } from "./ResultStrip";

// Inside BridgeInner, replace the current KqlBar mount with:
<KqlBar
  onRun={(q) => {
    dispatch({ type: "set-fleet-query", query: q });
    dispatch({ type: "set-fleet-result", result: runKqlStub(q) });
  }}
/>
<ResultStrip />
```

(`dispatch` comes from `useBridgeState()` — BridgeInner already has `state`; destructure `dispatch` too.)

- [ ] **Step 6: Commit**

```bash
pnpm typecheck && pnpm vitest run src/lib src/components/shell
# All tests pass
git add src/lib/kql-executor-stub.ts src/lib/kql-executor-stub.test.ts src/components/shell/ResultStrip.tsx src/components/shell/CommandBridge.tsx
git commit -m "feat(kql): stubbed executor + result strip"
```

---

## Task 15: Middle pane fleet mode

Replace the FleetList placeholder with a real flat match list driven by the stubbed executor's summary. Clicking a row pins the device in the rail and populates the right pane.

**Files:**
- Modify: `src/components/middle/FleetList.tsx`
- Create: `src/components/middle/FleetList.test.tsx`

- [ ] **Step 1: Write the test**

```tsx
// src/components/middle/FleetList.test.tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { BridgeStateProvider, useBridgeState } from "../../lib/bridge-state";
import { useEffect } from "react";

beforeEach(() => vi.resetModules());

async function loadFleet(pageItems: Array<{ sessionId: string; deviceId: string; parseState: string; ingestedUtc: string }>) {
  vi.doMock("../../lib/api-client", () => ({
    listDevices: async () => ({
      items: [...new Set(pageItems.map((x) => x.deviceId))].map((d) => ({
        deviceId: d,
        firstSeenUtc: "2026-01-01T00:00:00Z",
        lastSeenUtc: pageItems[0].ingestedUtc,
        hostname: d,
        sessionCount: 1,
      })),
      nextCursor: null,
    }),
    listSessions: async (_deviceId: string) => ({
      items: pageItems.filter((s) => s.deviceId === _deviceId).map((s) => ({
        sessionId: s.sessionId,
        deviceId: s.deviceId,
        bundleId: "b",
        collectedUtc: null,
        ingestedUtc: s.ingestedUtc,
        sizeBytes: 0,
        parseState: s.parseState,
      })),
      nextCursor: null,
    }),
  }));
  const { FleetList } = await import("./FleetList");
  return FleetList;
}

function Seed({ result }: { result: any }) {
  const { dispatch } = useBridgeState();
  useEffect(() => {
    dispatch({ type: "set-fleet-result", result });
  }, [result, dispatch]);
  return null;
}

describe("FleetList", () => {
  it("shows empty state when no fleet result is present", async () => {
    const FleetList = await loadFleet([]);
    render(
      <BridgeStateProvider>
        <FleetList />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/run a query/i)).toBeInTheDocument();
  });

  it("renders device rows when a fleet result is set", async () => {
    const FleetList = await loadFleet([
      { sessionId: "s1", deviceId: "GELL-A", parseState: "failed", ingestedUtc: "2026-04-24T00:00:00Z" },
      { sessionId: "s2", deviceId: "GELL-B", parseState: "partial", ingestedUtc: "2026-04-24T00:00:00Z" },
    ]);
    render(
      <BridgeStateProvider>
        <Seed result={{ matches: 2, devices: 2, sessions: 2, files: 0, groupBy: "device" }} />
        <FleetList />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText("GELL-A")).toBeInTheDocument());
    expect(screen.getByText("GELL-B")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Rewrite FleetList**

```tsx
// src/components/middle/FleetList.tsx
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
    if (!state.fleetResult) {
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
          if (enriched.length >= state.fleetResult.matches) break;
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
```

- [ ] **Step 3: Run tests + commit**

```bash
pnpm vitest run src/components/middle
# PASS — MiddlePane 2 + SessionTree 2 + FleetList 2
git add src/components/middle
git commit -m "feat(middle): fleet mode flat match list"
```

---

## Task 16: Keyboard shortcut registry + help overlay

A tiny shortcut registry hook + an overlay that lists all shortcuts when `?` is pressed. Covers the shortcuts already in the kbd strip plus `J`/`K` row navigation inside the grid.

**Files:**
- Create: `src/lib/keyboard-shortcuts.tsx`
- Create: `src/lib/keyboard-shortcuts.test.tsx`
- Create: `src/components/overlays/HelpOverlay.tsx`
- Create: `src/components/overlays/HelpOverlay.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — register shortcuts

- [ ] **Step 1: Write the hook test**

```tsx
// src/lib/keyboard-shortcuts.test.tsx
import { describe, it, expect, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useShortcut } from "./keyboard-shortcuts";

describe("useShortcut", () => {
  it("fires the handler on matching keydown", () => {
    const handler = vi.fn();
    renderHook(() => useShortcut({ key: "b", meta: true }, handler));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: true }));
    expect(handler).toHaveBeenCalledOnce();
  });

  it("ignores the event when meta modifier doesn't match", () => {
    const handler = vi.fn();
    renderHook(() => useShortcut({ key: "b", meta: true }, handler));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: false }));
    expect(handler).not.toHaveBeenCalled();
  });

  it("unregisters on unmount", () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useShortcut({ key: "b", meta: true }, handler));
    unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: true }));
    expect(handler).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Write the hook**

```tsx
// src/lib/keyboard-shortcuts.tsx
// Minimal global keyboard-shortcut hook. Use at any level in the tree.
// Matches are exact: meta/shift/alt flags must all line up. The hook does
// NOT debounce or intercept events inside <input>/<textarea> — callers
// that want field-aware shortcuts should branch in the handler.

import { useEffect } from "react";

export interface ShortcutSpec {
  key: string;
  meta?: boolean;
  shift?: boolean;
  alt?: boolean;
}

export function useShortcut(spec: ShortcutSpec, handler: (e: KeyboardEvent) => void) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key.toLowerCase() !== spec.key.toLowerCase()) return;
      if (!!spec.meta !== (e.metaKey || e.ctrlKey)) return;
      if (!!spec.shift !== e.shiftKey) return;
      if (!!spec.alt !== e.altKey) return;
      handler(e);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [spec.key, spec.meta, spec.shift, spec.alt, handler]);
}
```

- [ ] **Step 3: Run hook tests**

Run: `pnpm vitest run src/lib/keyboard-shortcuts.test.tsx`
Expected: PASS — 3 tests.

- [ ] **Step 4: Write HelpOverlay test**

```tsx
// src/components/overlays/HelpOverlay.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HelpOverlay } from "./HelpOverlay";

describe("HelpOverlay", () => {
  it("renders nothing when closed", () => {
    const { container } = render(<HelpOverlay open={false} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("lists the known shortcuts when open", () => {
    render(<HelpOverlay open={true} onClose={() => {}} />);
    expect(screen.getByText(/focus query/i)).toBeInTheDocument();
    expect(screen.getByText(/toggle rail/i)).toBeInTheDocument();
    expect(screen.getByText(/next file/i)).toBeInTheDocument();
  });

  it("fires onClose when the backdrop is clicked", () => {
    const onClose = vi.fn();
    render(<HelpOverlay open={true} onClose={onClose} />);
    fireEvent.click(screen.getByTestId("help-backdrop"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 5: Write HelpOverlay**

```tsx
// src/components/overlays/HelpOverlay.tsx
import { theme } from "../../lib/theme";

const SHORTCUTS: { keys: string; label: string }[] = [
  { keys: "⌘/", label: "focus query bar" },
  { keys: "⌘B", label: "toggle rail (collapse/expand)" },
  { keys: "⌘K", label: "jump to device search" },
  { keys: "⌘↑ / ⌘↓", label: "previous / next file in session" },
  { keys: "J / K", label: "row navigation in log grid" },
  { keys: "/", label: "focus log-grid search" },
  { keys: "Enter", label: "open row detail" },
  { keys: "?", label: "this help overlay" },
  { keys: "Esc", label: "close dropdown / overlay / dismiss" },
];

interface Props {
  open: boolean;
  onClose: () => void;
}

export function HelpOverlay({ open, onClose }: Props) {
  if (!open) return null;
  return (
    <div
      data-testid="help-backdrop"
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.55)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 100,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: theme.bg,
          border: `1px solid ${theme.border}`,
          borderRadius: 8,
          padding: "1.25rem 1.5rem",
          minWidth: "360px",
          fontFamily: theme.font.mono,
          fontSize: "0.78rem",
          color: theme.text,
        }}
      >
        <h2 style={{ margin: "0 0 0.75rem", color: theme.accent, fontSize: "0.7rem", letterSpacing: "0.15em", textTransform: "uppercase" }}>
          Keyboard shortcuts
        </h2>
        <dl style={{ display: "grid", gridTemplateColumns: "110px 1fr", gap: "0.3rem 1rem", margin: 0 }}>
          {SHORTCUTS.map(({ keys, label }) => (
            <div key={label} style={{ display: "contents" }}>
              <dt style={{ color: theme.accent }}>{keys}</dt>
              <dd style={{ margin: 0 }}>{label}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
```

- [ ] **Step 6: Register shortcuts in CommandBridge**

Edit `src/components/shell/CommandBridge.tsx`. Add at the top of `BridgeInner`:

```tsx
import { useState } from "react";
import { useShortcut } from "../../lib/keyboard-shortcuts";
import { HelpOverlay } from "../overlays/HelpOverlay";

function BridgeInner() {
  const { state, dispatch } = useBridgeState();
  const [helpOpen, setHelpOpen] = useState(false);

  // ⌘B — toggle rail
  useShortcut({ key: "b", meta: true }, (e) => { e.preventDefault(); dispatch({ type: "toggle-rail" }); });
  // ⌘/ — focus query bar (KqlBar exposes its own focus via a global id below)
  useShortcut({ key: "/", meta: true }, (e) => {
    e.preventDefault();
    document.getElementById("kql-input")?.focus();
  });
  // ? — help
  useShortcut({ key: "?" }, (e) => {
    // Ignore when typing in an input.
    if (document.activeElement?.tagName === "INPUT") return;
    e.preventDefault();
    setHelpOpen(true);
  });
  // Esc — close help
  useShortcut({ key: "Escape" }, () => setHelpOpen(false));

  // ...rest of BridgeInner unchanged, with HelpOverlay rendered at the end:
  return (
    <>
      <div style={{ /* ...existing root container... */ }}>
        {/* existing regions */}
      </div>
      <HelpOverlay open={helpOpen} onClose={() => setHelpOpen(false)} />
    </>
  );
}
```

And in `KqlBar.tsx`, add `id="kql-input"` to the `<input>` so `⌘/` can focus it.

- [ ] **Step 7: Run tests + commit**

```bash
pnpm typecheck && pnpm vitest run src/lib/keyboard-shortcuts.test.tsx src/components/overlays
# PASS — shortcut 3 + help 3
git add src/lib/keyboard-shortcuts.tsx src/lib/keyboard-shortcuts.test.tsx src/components/overlays src/components/shell
git commit -m "feat(shell): global shortcut hook + help overlay (?)"
```

---

## Task 17: Local-mode overlay (drag-drop + ⌘O)

Drag any `.log` / `.cmtlog` / `.txt` file onto the shell, or press `⌘O`, to open LocalMode inside an overlay. Reuses the existing LocalMode component (no rewrite — just wraps it).

**Files:**
- Create: `src/components/overlays/LocalOverlay.tsx`
- Create: `src/components/overlays/LocalOverlay.test.tsx`
- Modify: `src/components/shell/CommandBridge.tsx` — add global drag handler + shortcut, mount the overlay

- [ ] **Step 1: Write the test**

```tsx
// src/components/overlays/LocalOverlay.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// Stub LocalMode so the test is independent of the real component's deps.
vi.mock("../LocalMode", () => ({
  __esModule: true,
  LocalMode: () => <div data-testid="local-mode">local-mode</div>,
}));

import { LocalOverlay } from "./LocalOverlay";

describe("LocalOverlay", () => {
  it("renders nothing when closed", () => {
    const { container } = render(<LocalOverlay open={false} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders LocalMode when open", () => {
    render(<LocalOverlay open={true} onClose={() => {}} />);
    expect(screen.getByTestId("local-mode")).toBeInTheDocument();
  });

  it("fires onClose on Esc", () => {
    const onClose = vi.fn();
    render(<LocalOverlay open={true} onClose={onClose} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Write LocalOverlay**

```tsx
// src/components/overlays/LocalOverlay.tsx
import { useEffect } from "react";
import { LocalMode } from "../LocalMode";
import { theme } from "../../lib/theme";

interface Props {
  open: boolean;
  onClose: () => void;
}

export function LocalOverlay({ open, onClose }: Props) {
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: theme.bg,
        zIndex: 200,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <header style={{ padding: "0.5rem 0.75rem", borderBottom: `1px solid ${theme.border}`, display: "flex", justifyContent: "space-between", alignItems: "center", fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        <span style={{ color: theme.accent }}>LOCAL · FILE</span>
        <button
          type="button"
          onClick={onClose}
          style={{ all: "unset", color: theme.textDim, cursor: "pointer" }}
        >
          Esc · close
        </button>
      </header>
      <div style={{ flex: 1, overflow: "auto" }}>
        <LocalMode />
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Mount in CommandBridge with drag + ⌘O handlers**

Edit `src/components/shell/CommandBridge.tsx`:

```tsx
import { LocalOverlay } from "../overlays/LocalOverlay";

// Inside BridgeInner, add state and shortcut:
const [localOpen, setLocalOpen] = useState(false);
useShortcut({ key: "o", meta: true }, (e) => { e.preventDefault(); setLocalOpen(true); });

// Wrap the top-level div with drag handlers:
<div
  onDragOver={(e) => { e.preventDefault(); }}
  onDrop={(e) => {
    const file = e.dataTransfer.files?.[0];
    if (!file) return;
    const name = file.name.toLowerCase();
    if (!/\.(log|cmtlog|txt)$/.test(name)) return;
    e.preventDefault();
    setLocalOpen(true);
    // LocalMode's file-picker path reads the dropped file via its own
    // internal DropZone component. Passing the file directly would require
    // exposing a new prop on LocalMode; for v1 we let LocalMode's own
    // drop handler pick it up on the overlay's first render.
  }}
  style={{ /* existing */ }}
>
  {/* existing body */}
</div>

// At the end:
<LocalOverlay open={localOpen} onClose={() => setLocalOpen(false)} />
```

- [ ] **Step 4: Run tests + commit**

```bash
pnpm typecheck && pnpm vitest run src/components/overlays
# PASS — HelpOverlay 3 + LocalOverlay 3
git add src/components/overlays/LocalOverlay.tsx src/components/overlays/LocalOverlay.test.tsx src/components/shell/CommandBridge.tsx
git commit -m "feat(shell): LocalMode overlay (drag-drop + ⌘O)"
```

---

## Task 18: Cutover — delete legacy, flip default

Old shell goes away. New shell becomes the default. `?v=next` gate inverts (or is removed entirely).

**Files:**
- Modify: `src/main.tsx` — flip default
- Delete: `src/components/ViewerShell.tsx`, `ApiMode.tsx`, `DeviceLogViewer.tsx`, `FilesPanel.tsx`
- Delete: `src/components/layout/Toolbar.tsx`, `FileSidebar.tsx`, `TabStrip.tsx` (if unused after cutover)
- Delete: `src/components/FilterBar.tsx` (superseded by `src/components/right/FilterBar.tsx`)
- Delete: `src/lib/workspace-context.tsx` if no callers remain (grep first)
- Modify: any remaining imports that referenced deleted modules

- [ ] **Step 1: Audit callers**

Run these greps before deleting. If any match in `src/` outside the files being deleted, fix the caller first:

```bash
grep -rln "ViewerShell" src/
grep -rln "ApiMode" src/
grep -rln "DeviceLogViewer" src/
grep -rln "FilesPanel" src/
grep -rln "from.*layout/Toolbar" src/
grep -rln "from.*layout/FileSidebar" src/
grep -rln "from.*layout/TabStrip" src/
grep -rln "workspace-context" src/
grep -rln "from .\./FilterBar\"" src/ src/components
```

Expected: the only callers are inside the files being deleted, or inside `src/main.tsx` (for ViewerShell).

- [ ] **Step 2: Flip main.tsx default**

Edit `src/main.tsx`:

```tsx
// Replace the gate:
import { CommandBridge } from "./components/shell/CommandBridge";
// Remove: import ViewerShell.

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <CommandBridge />
  </React.StrictMode>
);
```

- [ ] **Step 3: Delete legacy files**

```bash
git rm src/components/ViewerShell.tsx
git rm src/components/ApiMode.tsx
git rm src/components/DeviceLogViewer.tsx
git rm src/components/FilesPanel.tsx
git rm src/components/FilterBar.tsx     # superseded by right/FilterBar
git rm src/components/layout/Toolbar.tsx
git rm src/components/layout/FileSidebar.tsx
git rm src/components/layout/TabStrip.tsx   # if audit confirmed unused
# workspace-context only if the audit returned zero callers:
git rm src/lib/workspace-context.tsx
```

- [ ] **Step 4: Verify build + tests are still green**

Run: `pnpm typecheck && pnpm test && pnpm build`
Expected: typecheck clean, all tests pass, production build succeeds.

If typecheck surfaces errors (most likely: a deleted file was re-exported somewhere), fix the import in the caller. Do NOT resurrect the deleted file.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(shell): cutover to command-bridge; delete legacy shell"
```

---

## Task 19: Token adoption on LocalMode + DiffView

Swap Fluent token references for our theme tokens so they stop looking out of place next to the new shell. Keep all functionality intact.

**Files:**
- Modify: `src/components/LocalMode.tsx`
- Modify: `src/components/log-view/DiffView.tsx`
- Modify: any other `src/components/log-view/*` files that render chrome (e.g., `MergeLegendBar.tsx`, `DnsWorkspaceBanner.tsx`)

- [ ] **Step 1: Audit Fluent token usage in the target files**

```bash
grep -n "tokens\." src/components/LocalMode.tsx src/components/log-view/*.tsx
```

For each match, replace with the nearest equivalent from `theme.ts`:

| Fluent token | Replacement |
|---|---|
| `tokens.colorNeutralBackground1` | `theme.bg` |
| `tokens.colorNeutralBackground2` | `theme.surface` |
| `tokens.colorNeutralBackground3` | `theme.surfaceAlt` |
| `tokens.colorNeutralForeground1` | `theme.textPrimary` |
| `tokens.colorNeutralForeground2` | `theme.text` |
| `tokens.colorNeutralForeground3` / `4` | `theme.textDim` / `theme.textFainter` |
| `tokens.colorNeutralStroke1` / `2` | `theme.border` |
| `tokens.colorBrandBackground` / `Foreground` | `theme.accent` / `theme.accentBg` |
| `tokens.fontFamilyMonospace` | `theme.font.mono` |
| `tokens.fontFamilyBase` | `theme.font.ui` |

- [ ] **Step 2: Replace tokens file-by-file**

For `src/components/LocalMode.tsx`:

Remove the `import { tokens } from "@fluentui/react-components"` line (keep any Button/Spinner imports). Add `import { theme } from "../lib/theme";`. Do a find-and-replace per the table above.

Repeat for every file listed by the grep in Step 1.

- [ ] **Step 3: Verify tests + a manual render**

Run: `pnpm typecheck && pnpm test`

Then: `pnpm dev`, navigate to the shell, open a local file via ⌘O, load a bundle in Diff view if it's reachable. Confirm LocalMode and DiffView look consistent with the new shell (dark bg, teal accent, monospace metadata).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "style(viewer): adopt theme tokens in LocalMode + DiffView"
```

---

## Post-plan follow-ups (separate specs)

The spec §scope explicitly defers these. Do NOT attempt here:

- **mTLS hardening + testing** — next spec.
- **Real KQL executor** — replaces `kql-executor-stub.ts`. Needs a parser + either client-side REST translation or a new `/v1/query` endpoint.
- **Device-health bulk endpoint** — replaces the N+1 `listSessions(limit=1)` calls in DeviceRail.
- **Server-persisted saved views** — replaces the localStorage path in SavedViews / KqlBar.
- **Visual regression testing harness** — Percy / Chromatic / Storybook snapshots.
- **Theme light mode** — paired with a tokens redesign.

---

## Self-review

**1. Spec coverage:**

| Spec section | Covered in |
|---|---|
| §1 Shell architecture | Tasks 2 (bridge state), 3 (skeleton), 16 (shortcuts) |
| §2 KQL bar | Tasks 12, 13, 14 |
| §3 Banner | Task 4 |
| §4 Left rail | Tasks 5, 6, 7 |
| §5 Middle pane | Tasks 8, 15 |
| §6 Right pane | Tasks 9, 10, 11 |
| §7 Migration | Task 18 |
| §8 Theme tokens | Task 1 |
| §9 Error handling | Tasks 6 (rail), 8 (tree), 9/10 (viewer), 13 (parse error visual – deferred to real executor) |
| §10 Testing | Every task writes component + integration-ish tests; a11y + integration covered in individual tasks |
| Build order | 11 → 19 tasks expanded 1:1 with additional sub-granularity |
| Open questions | Listed in post-plan follow-ups |

All spec sections map to a task.

**2. Placeholder scan:**

No "TBD", "implement later", "handle edge cases" appear. Every step has concrete code, exact file paths, and commit commands. The one `TODO(real-executor)` comment inside `kql-executor-stub.ts` is a forward-reference (real executor is explicitly a separate spec), not a placeholder in this plan's prescribed work.

**3. Type consistency:**

- `BridgeState` / `BridgeAction` defined in Task 2; consumed by Tasks 6, 8, 9, 13, 14, 15, 16, 17 — field names and action types match.
- `PillState` imported from `theme.ts` (Task 1) and used in Tasks 4, 5, 6, 8, 15 — same type throughout.
- `RailDevice` (Task 6 DeviceRow) vs `BannerDevice` (Task 4) are deliberately distinct shapes — the rail only needs id+last-seen+health; the banner needs additional totals. Both feed from the same `DeviceSummary` but at different aggregation stages.
- `LogEntry` / `LogEntryDto` continue to come from the existing `src/lib/log-types.ts` and `src/lib/dto-to-entry.ts` — no new wire types.
- `FleetResultSummary` defined in Task 2, produced by Task 14's stub, consumed by Task 15 FleetList — shape matches.
- `Filters` (Task 10 FilterBar) is used only by LogViewer in Task 10; no external consumers.

No mismatches found.

---

**Plan complete and saved to `docs/superpowers/plans/2026-04-24-viewer-command-bridge.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
