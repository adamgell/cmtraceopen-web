// Tests for the DeviceRail — the list-of-devices left rail.
//
// Two cases:
//   1. Happy path: listDevices returns devices, per-device listSessions seeds
//      the health dot, the rail renders a DeviceRow for each.
//   2. Error path: listDevices rejects — rail shows a `fleet unreachable`
//      banner with a retry button.
//
// The api-client module is mocked per-test with `vi.doMock` + `vi.resetModules()`
// (same pattern as RoleGate.test.tsx). Because resetModules invalidates the
// bridge-state module too, the BridgeStateProvider is re-imported inside the
// loader — otherwise DeviceRail's useBridgeState() hits a different module
// instance than the outer provider.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

beforeEach(() => vi.resetModules());

async function loadRail(mock: {
  listDevices: () => Promise<unknown>;
  listSessions: () => Promise<unknown>;
}) {
  vi.doMock("../../lib/api-client", () => mock);
  const { DeviceRail } = await import("./DeviceRail");
  const { BridgeStateProvider } = await import("../../lib/bridge-state");
  return { DeviceRail, BridgeStateProvider };
}

describe("DeviceRail", () => {
  it("renders devices returned by the api client", async () => {
    const devices = [
      { deviceId: "GELL-01AA310", lastSeenUtc: new Date(Date.now() - 60_000).toISOString(), sessionCount: 44 },
    ];
    const { DeviceRail, BridgeStateProvider } = await loadRail({
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
    });
    render(
      <BridgeStateProvider>
        <DeviceRail />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText("GELL")).toBeInTheDocument());
  });

  it("shows a retry banner when the listDevices call rejects", async () => {
    const { DeviceRail, BridgeStateProvider } = await loadRail({
      listDevices: async () => { throw new Error("fleet down"); },
      listSessions: async () => ({ items: [], nextCursor: null }),
    });
    render(
      <BridgeStateProvider>
        <DeviceRail />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/fleet unreachable/i)).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
