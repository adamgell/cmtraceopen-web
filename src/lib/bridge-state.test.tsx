import { describe, it, expect, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { BridgeStateProvider, useBridgeState } from "./bridge-state";
import type { ReactNode } from "react";

function wrapper({ children }: { children: ReactNode }) {
  return <BridgeStateProvider>{children}</BridgeStateProvider>;
}

// The harness-provided `localStorage` in this test env is an empty object
// (no `getItem`/`setItem`/`removeItem`/`clear`). Install a Map-backed shim so
// the persistence contract tests below can observe real reads/writes. Scoped
// to this test file via `beforeEach`; other tests continue to see the
// inert stub and fall through `bridge-state.tsx`'s try/catch.
function installLocalStorageShim() {
  const store = new Map<string, string>();
  const shim = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => {
      store.set(k, String(v));
    },
    removeItem: (k: string) => {
      store.delete(k);
    },
    clear: () => {
      store.clear();
    },
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  };
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: shim,
  });
}

describe("bridge state", () => {
  it("starts with rail expanded and no selection", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    expect(result.current.state.railExpanded).toBe(true);
    expect(result.current.state.selectedDeviceId).toBeNull();
    expect(result.current.state.middleMode).toBe("device");
  });

  it("toggles the rail", () => {
    const { result } = renderHook(() => useBridgeState(), { wrapper });
    act(() => result.current.dispatch({ type: "toggle-rail" }));
    expect(result.current.state.railExpanded).toBe(false);
    act(() => result.current.dispatch({ type: "toggle-rail" }));
    expect(result.current.state.railExpanded).toBe(true);
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

  describe("rail persistence", () => {
    beforeEach(() => {
      installLocalStorageShim();
    });

    it("persists rail expanded to localStorage on toggle", () => {
      localStorage.clear();
      const { result } = renderHook(() => useBridgeState(), { wrapper });
      // Default state is expanded — first toggle flips to collapsed.
      act(() => result.current.dispatch({ type: "toggle-rail" }));
      expect(localStorage.getItem("cmtrace.rail-expanded.v2")).toBe("0");
      act(() => result.current.dispatch({ type: "toggle-rail" }));
      expect(localStorage.getItem("cmtrace.rail-expanded.v2")).toBe("1");
    });

    it("initializes rail expanded from localStorage on mount", () => {
      localStorage.clear();
      localStorage.setItem("cmtrace.rail-expanded.v2", "1");
      const { result } = renderHook(() => useBridgeState(), { wrapper });
      expect(result.current.state.railExpanded).toBe(true);
    });
  });
});
