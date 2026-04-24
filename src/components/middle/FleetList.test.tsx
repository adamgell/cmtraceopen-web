// Tests for FleetList — fleet-mode body of the middle pane.
//
// Follows the same vi.doMock + resetModules + dynamic-import pattern as
// SessionTree.test.tsx so the FleetList's useBridgeState() and the
// BridgeStateProvider resolve to the same module instance (otherwise the
// context lookup fails under the Vite/Vitest module graph).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";

beforeEach(() => vi.resetModules());

async function loadFleet(pageItems: Array<{ sessionId: string; deviceId: string; parseState: string; ingestedUtc: string }>) {
  vi.doMock("../../lib/api-client", () => ({
    listDevices: async () => ({
      items: [...new Set(pageItems.map((x) => x.deviceId))].map((d) => ({
        deviceId: d,
        firstSeenUtc: "2026-01-01T00:00:00Z",
        lastSeenUtc: pageItems[0]?.ingestedUtc ?? "2026-04-24T00:00:00Z",
        hostname: d,
        sessionCount: 1,
      })),
      nextCursor: null,
    }),
    listSessions: async (deviceId: string) => ({
      items: pageItems.filter((s) => s.deviceId === deviceId).map((s) => ({
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
  const { BridgeStateProvider, useBridgeState } = await import("../../lib/bridge-state");
  return { FleetList, BridgeStateProvider, useBridgeState };
}

describe("FleetList", () => {
  it("shows empty state when no fleet result is present", async () => {
    const { FleetList, BridgeStateProvider } = await loadFleet([]);
    render(
      <BridgeStateProvider>
        <FleetList />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/run a query/i)).toBeInTheDocument();
  });

  it("renders device rows when a fleet result is set", async () => {
    const { FleetList, BridgeStateProvider, useBridgeState } = await loadFleet([
      { sessionId: "s1", deviceId: "GELL-A", parseState: "failed", ingestedUtc: "2026-04-24T00:00:00Z" },
      { sessionId: "s2", deviceId: "GELL-B", parseState: "partial", ingestedUtc: "2026-04-24T00:00:00Z" },
    ]);

    function Seed({ result }: { result: any }) {
      const { dispatch } = useBridgeState();
      useEffect(() => {
        dispatch({ type: "set-fleet-result", result });
      }, [result, dispatch]);
      return null;
    }

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
