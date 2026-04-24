import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";

beforeEach(() => vi.resetModules());

async function loadViewer(entriesPage: any) {
  vi.doMock("../../lib/api-client", () => ({
    listEntries: async () => entriesPage,
  }));
  const { LogViewer } = await import("./LogViewer");
  const { BridgeStateProvider, useBridgeState } = await import("../../lib/bridge-state");
  return { LogViewer, BridgeStateProvider, useBridgeState };
}

describe("LogViewer", () => {
  it("renders an empty-state message when no file is selected", async () => {
    const { LogViewer, BridgeStateProvider } = await loadViewer({ items: [], nextCursor: null });
    render(
      <BridgeStateProvider>
        <LogViewer />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/pick a file/i)).toBeInTheDocument();
  });

  it("fetches and renders entries when a file is selected via bridge state", async () => {
    const { LogViewer, BridgeStateProvider, useBridgeState } = await loadViewer({
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
      const { dispatch } = useBridgeState();
      useEffect(() => {
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
