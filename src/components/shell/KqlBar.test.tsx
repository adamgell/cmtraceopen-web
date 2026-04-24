import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";
import { KqlBar } from "./KqlBar";

// jsdom's localStorage in this test env is an empty stub missing
// getItem/setItem. Install a Map-backed shim so the recent-queries persistence
// path hits a real store. Scoped to this file via beforeEach.
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

beforeEach(() => {
  installLocalStorageShim();
  localStorage.clear();
  vi.restoreAllMocks();
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
