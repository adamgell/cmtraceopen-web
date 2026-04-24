import { describe, it, expect, vi } from "vitest";
import { act, render, screen, within } from "@testing-library/react";
import { CommandBridge } from "./CommandBridge";

// CommandBridge mounts DeviceRail + MiddlePane, which call listDevices,
// listSessions, and listFiles. These tests assert on the static shell layout
// only — stub the client so the fetches resolve to an empty list. Each test
// awaits one microtask tick inside act() so React can flush the settled state
// update before we assert, avoiding the `not wrapped in act(...)` warning.
vi.mock("../../lib/api-client", () => ({
  listDevices: async () => ({ items: [], nextCursor: null }),
  listSessions: async () => ({ items: [], nextCursor: null }),
  listFiles: async () => ({ items: [], nextCursor: null }),
}));

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("CommandBridge skeleton", () => {
  it("renders the four shell regions", async () => {
    render(<CommandBridge />);
    await flush();
    expect(screen.getByTestId("kql-bar")).toBeInTheDocument();
    expect(screen.getByTestId("banner")).toBeInTheDocument();
    expect(screen.getByTestId("rail")).toBeInTheDocument();
    expect(screen.getByTestId("middle-pane")).toBeInTheDocument();
    expect(screen.getByTestId("right-pane")).toBeInTheDocument();
    expect(screen.getByTestId("status-bar")).toBeInTheDocument();
  });

  it("defaults the grid to the collapsed rail width (56px)", async () => {
    render(<CommandBridge />);
    await flush();
    const rail = screen.getByTestId("rail");
    // Rail is the first grid-track in its parent's columns template.
    const track = rail.parentElement!.style.gridTemplateColumns;
    expect(track).toMatch(/^56px\s+220px\s+1fr$/);
  });

  it("nests the status bar inside the right pane", async () => {
    const { getByTestId } = render(<CommandBridge />);
    await flush();
    const rightPane = getByTestId("right-pane");
    // Must be nested — not a sibling of right-pane.
    expect(within(rightPane).getByTestId("status-bar")).toBeInTheDocument();
  });
});
