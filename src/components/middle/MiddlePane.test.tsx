// Tests for MiddlePane — the tabbed container that routes between device
// mode (SessionTree) and fleet mode (FleetList).
//
// Children are mocked here so the harness isn't coupled to their internals;
// SessionTree.test.tsx covers the device-mode body end-to-end.

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { BridgeStateProvider } from "../../lib/bridge-state";

vi.mock("./SessionTree", () => ({ SessionTree: ({ deviceId }: { deviceId: string }) => <div data-testid="tree">tree-{deviceId}</div> }));
vi.mock("./FleetList", () => ({ FleetList: () => <div data-testid="fleet">fleet-list</div> }));

import { MiddlePane } from "./MiddlePane";

describe("MiddlePane", () => {
  it("shows empty copy when no device is selected and mode is device", () => {
    render(
      <BridgeStateProvider>
        <MiddlePane />
      </BridgeStateProvider>
    );
    expect(screen.getByText(/pick a device/i)).toBeInTheDocument();
  });

  it("switches to fleet when the fleet tab is clicked", () => {
    render(
      <BridgeStateProvider>
        <MiddlePane />
      </BridgeStateProvider>
    );
    fireEvent.click(screen.getByRole("button", { name: /FLEET/i }));
    expect(screen.getByTestId("fleet")).toBeInTheDocument();
  });
});
