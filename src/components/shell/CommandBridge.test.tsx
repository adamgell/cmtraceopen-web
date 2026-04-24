import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
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

  it("defaults the rail width to the collapsed size (56px)", () => {
    render(<CommandBridge />);
    const rail = screen.getByTestId("rail");
    expect(rail.style.width).toBe("56px");
  });
});
