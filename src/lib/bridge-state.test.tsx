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
