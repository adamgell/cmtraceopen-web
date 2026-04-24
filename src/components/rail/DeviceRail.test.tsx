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
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

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

  it("re-runs listDevices when the retry button is clicked", async () => {
    let listCalls = 0;
    vi.doMock("../../lib/api-client", () => ({
      listDevices: async () => {
        listCalls++;
        if (listCalls === 1) throw new Error("fleet down");
        return {
          items: [{
            deviceId: "GELL-01AA310",
            firstSeenUtc: new Date().toISOString(),
            lastSeenUtc: new Date().toISOString(),
            hostname: "GELL-01AA310",
            sessionCount: 1,
          }],
          nextCursor: null,
        };
      },
      listSessions: async () => ({
        items: [{
          sessionId: "s1", deviceId: "GELL-01AA310", bundleId: "b",
          collectedUtc: null, ingestedUtc: new Date().toISOString(),
          sizeBytes: 0, parseState: "ok",
        }],
        nextCursor: null,
      }),
    }));
    const { DeviceRail } = await import("./DeviceRail");
    const { BridgeStateProvider: Provider } = await import("../../lib/bridge-state");
    render(
      <Provider>
        <DeviceRail />
      </Provider>
    );
    // Wait for the error banner to appear (first call failed).
    await waitFor(() => expect(screen.getByText(/fleet unreachable/i)).toBeInTheDocument());
    // Click retry → second fetch succeeds → device renders.
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await waitFor(() => expect(screen.getByText("GELL")).toBeInTheDocument());
    expect(listCalls).toBe(2);
  });

  it("degrades one failing device to pending without sinking siblings", async () => {
    vi.doMock("../../lib/api-client", () => ({
      listDevices: async () => ({
        items: [
          { deviceId: "GELL-AAA", firstSeenUtc: "2026-01-01T00:00:00Z", lastSeenUtc: new Date(Date.now() - 60_000).toISOString(), hostname: "a", sessionCount: 1 },
          { deviceId: "GELL-BBB", firstSeenUtc: "2026-01-01T00:00:00Z", lastSeenUtc: new Date(Date.now() - 60_000).toISOString(), hostname: "b", sessionCount: 1 },
        ],
        nextCursor: null,
      }),
      listSessions: async (deviceId: string) => {
        if (deviceId === "GELL-AAA") throw new Error("session-index down for A");
        return {
          items: [{
            sessionId: "b-s1", deviceId: "GELL-BBB", bundleId: "b",
            collectedUtc: null, ingestedUtc: new Date().toISOString(),
            sizeBytes: 0, parseState: "ok",
          }],
          nextCursor: null,
        };
      },
    }));
    const { DeviceRail } = await import("./DeviceRail");
    const { BridgeStateProvider: Provider } = await import("../../lib/bridge-state");
    render(
      <Provider>
        <DeviceRail />
      </Provider>
    );
    // Both devices still render — A is "pending"/gray, B renders normally.
    await waitFor(() => {
      const rows = screen.getAllByRole("button").filter((el) => el.getAttribute("title")?.startsWith("GELL-"));
      expect(rows.length).toBe(2);
    });
  });
});
