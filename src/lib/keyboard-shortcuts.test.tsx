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
