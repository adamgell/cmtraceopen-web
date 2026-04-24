import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { StatusBar } from "./StatusBar";

describe("StatusBar", () => {
  it("renders row counts + severity totals", () => {
    render(<StatusBar rendered={28} limit={500} total={1_300_000} warnCount={2} errCount={2} />);
    expect(screen.getByText(/28 \/ 500/)).toBeInTheDocument();
    expect(screen.getByText(/1\.3M total/)).toBeInTheDocument();
    expect(screen.getByText(/2 warn/)).toBeInTheDocument();
    expect(screen.getByText(/2 err/)).toBeInTheDocument();
  });
});
