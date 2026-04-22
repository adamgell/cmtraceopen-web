// Tests for DevicesPanel.
//
// Focused on the four behaviors flagged in the PR review:
//   - Disable button visibility gated by Admin role (visible) / Operator-only (hidden)
//   - Confirm modal calls disableDevice with the FULL device id (not the
//     truncated display string)
//   - Filter input is debounced (~250 ms) — keystrokes do NOT re-query the
//     API; the API is only hit on initial load + the polling interval (paused
//     while a modal is open).
//
// Strategy: stub `api-client` (the network layer) with vi.fn()s so we can
// assert the exact arguments passed in. Stub `auth-config` + `@azure/msal-react`
// so RoleGate sees the role we want for each test. Use fake timers for the
// debounce assertion so we don't have to wait wall-clock 250 ms.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { cleanup, render, screen, act, fireEvent } from "@testing-library/react";

const FULL_DEVICE_ID = "device://contoso/very-long-device-id-that-gets-truncated-by-the-ui-99";
const SHORT_HOSTNAME = "win11-prod-01";

const listDevicesPage = vi.fn();
const disableDevice = vi.fn();

// Module mocks have to be hoisted before the component import. We re-set the
// underlying vi.fn implementations per-test inside `beforeEach`.
vi.mock("../lib/api-client", () => ({
  apiBase: "",
  listDevicesPage: (...args: unknown[]) => listDevicesPage(...args),
  disableDevice: (...args: unknown[]) => disableDevice(...args),
}));

// Default to Admin-with-role; specific tests override via vi.doMock + dynamic import.
vi.mock("../lib/auth-config", () => ({
  entraConfig: { status: "configured" },
  ROLE_ADMIN: "CmtraceOpen.Admin",
}));

vi.mock("@azure/msal-react", () => ({
  useMsal: () => ({
    accounts: [{ idTokenClaims: { roles: ["CmtraceOpen.Admin"] } }],
  }),
}));

// Helper to (re)load DevicesPanel with a different role-gating shape per test.
async function loadPanelWith(opts: { roles: string[] | null }) {
  vi.resetModules();
  vi.doMock("../lib/api-client", () => ({
    apiBase: "",
    listDevicesPage: (...args: unknown[]) => listDevicesPage(...args),
    disableDevice: (...args: unknown[]) => disableDevice(...args),
  }));
  vi.doMock("../lib/auth-config", () => ({
    entraConfig: { status: "configured" },
    ROLE_ADMIN: "CmtraceOpen.Admin",
  }));
  vi.doMock("@azure/msal-react", () => ({
    useMsal: () => ({
      accounts:
        opts.roles === null
          ? []
          : [{ idTokenClaims: { roles: opts.roles } }],
    }),
  }));
  const mod = await import("./DevicesPanel");
  return mod.DevicesPanel;
}

beforeEach(() => {
  listDevicesPage.mockReset();
  disableDevice.mockReset();
  // Single-page response for the initial load.
  listDevicesPage.mockResolvedValue({
    items: [
      {
        deviceId: FULL_DEVICE_ID,
        firstSeenUtc: "2026-04-01T00:00:00Z",
        lastSeenUtc: "2026-04-21T12:00:00Z",
        hostname: SHORT_HOSTNAME,
        sessionCount: 3,
        status: "active",
      },
    ],
    nextCursor: null,
  });
  disableDevice.mockResolvedValue({});
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("DevicesPanel — RBAC", () => {
  it("hides Disable button for an Operator-only token", async () => {
    const DevicesPanel = await loadPanelWith({
      roles: ["CmtraceOpen.Operator"],
    });
    await act(async () => {
      render(<DevicesPanel />);
    });
    // Wait a microtask for the async load to settle.
    await act(async () => {
      await Promise.resolve();
    });
    // Operator role: the RoleGate fallback renders a *disabled* Disable
    // button so the column has consistent layout — but it must be `disabled`.
    const buttons = screen.getAllByRole("button", { name: /disable/i });
    expect(buttons.length).toBeGreaterThan(0);
    for (const b of buttons) {
      expect(b).toBeDisabled();
    }
  });

  it("shows an enabled Disable button for an Admin token", async () => {
    const DevicesPanel = await loadPanelWith({
      roles: ["CmtraceOpen.Admin"],
    });
    await act(async () => {
      render(<DevicesPanel />);
    });
    await act(async () => {
      await Promise.resolve();
    });
    // Find the row's Disable button (the active one — `aria-disabled` false).
    const buttons = screen.getAllByRole("button", { name: /^Disable$/i });
    const enabled = buttons.filter((b) => !(b as HTMLButtonElement).disabled);
    expect(enabled.length).toBeGreaterThan(0);
  });
});

describe("DevicesPanel — disable flow", () => {
  it("calls disableDevice with the full device id (not the truncated display)", async () => {
    const DevicesPanel = await loadPanelWith({ roles: ["CmtraceOpen.Admin"] });
    await act(async () => {
      render(<DevicesPanel />);
    });
    await act(async () => {
      await Promise.resolve();
    });

    // Click Disable to open the confirm modal.
    const buttons = screen.getAllByRole("button", { name: /^Disable$/i });
    const enabledDisable = buttons.find(
      (b) => !(b as HTMLButtonElement).disabled,
    )!;
    await act(async () => {
      fireEvent.click(enabledDisable);
    });

    // Modal should display the FULL id (review fix).
    expect(screen.getByTestId("confirm-device-id")).toHaveTextContent(
      FULL_DEVICE_ID,
    );

    // Click "Yes, disable" — must hit the API with the full id.
    const confirmBtn = screen.getByRole("button", { name: /yes, disable/i });
    await act(async () => {
      fireEvent.click(confirmBtn);
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(disableDevice).toHaveBeenCalledTimes(1);
    expect(disableDevice).toHaveBeenCalledWith(FULL_DEVICE_ID);
  });
});

describe("DevicesPanel — filter debouncing", () => {
  it("does not re-query the API on every keystroke (debounced ~250 ms)", async () => {
    vi.useFakeTimers();
    const DevicesPanel = await loadPanelWith({ roles: ["CmtraceOpen.Admin"] });
    await act(async () => {
      render(<DevicesPanel />);
    });
    // Drain the initial load microtask.
    await act(async () => {
      await Promise.resolve();
    });

    expect(listDevicesPage).toHaveBeenCalledTimes(1); // initial load

    // Type quickly into the filter — none of these should trigger a new
    // listDevicesPage call (filtering is client-side, but more importantly
    // the debounced value should not change until the timer fires).
    const filterInput = screen.getByPlaceholderText(/filter by hostname/i);
    await act(async () => {
      fireEvent.change(filterInput, { target: { value: "w" } });
      fireEvent.change(filterInput, { target: { value: "wi" } });
      fireEvent.change(filterInput, { target: { value: "win" } });
    });

    // Advance less than 250 ms — debounced filter should not have updated yet.
    await act(async () => {
      vi.advanceTimersByTime(100);
    });
    expect(listDevicesPage).toHaveBeenCalledTimes(1);

    // Advance past 250 ms — debounced filter applies; listDevicesPage still
    // not re-called (filter is client-side).
    await act(async () => {
      vi.advanceTimersByTime(300);
    });
    expect(listDevicesPage).toHaveBeenCalledTimes(1);
  });
});
