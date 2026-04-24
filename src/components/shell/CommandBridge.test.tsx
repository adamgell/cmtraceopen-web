import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { CommandBridge } from "./CommandBridge";

describe("CommandBridge skeleton", () => {
  it("renders the four shell regions", () => {
    render(<CommandBridge />);
    expect(screen.getByTestId("kql-bar")).toBeInTheDocument();
    expect(screen.getByTestId("banner")).toBeInTheDocument();
    expect(screen.getByTestId("rail")).toBeInTheDocument();
    expect(screen.getByTestId("middle-pane")).toBeInTheDocument();
    expect(screen.getByTestId("right-pane")).toBeInTheDocument();
    expect(screen.getByTestId("status-bar")).toBeInTheDocument();
  });

  it("defaults the grid to the collapsed rail width (56px)", () => {
    render(<CommandBridge />);
    const rail = screen.getByTestId("rail");
    // Rail is the first grid-track in its parent's columns template.
    const track = rail.parentElement!.style.gridTemplateColumns;
    expect(track).toMatch(/^56px\s+220px\s+1fr$/);
  });

  it("nests the status bar inside the right pane", () => {
    const { getByTestId } = render(<CommandBridge />);
    const rightPane = getByTestId("right-pane");
    // Must be nested — not a sibling of right-pane.
    expect(within(rightPane).getByTestId("status-bar")).toBeInTheDocument();
  });
});
