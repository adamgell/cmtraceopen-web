import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SavedViews } from "./SavedViews";

// jsdom's localStorage in this test env is an empty stub missing getItem/setItem.
// Install a Map-backed shim so the SavedViews read/write path hits a real store.
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
