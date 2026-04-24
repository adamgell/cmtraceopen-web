import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { EntryGrid } from "./EntryGrid";

describe("EntryGrid", () => {
  it("renders a header row with the six column titles", () => {
    render(<EntryGrid entries={[]} />);
    expect(screen.getByText("LINE")).toBeInTheDocument();
    expect(screen.getByText("TIMESTAMP")).toBeInTheDocument();
    expect(screen.getByText("COMPONENT")).toBeInTheDocument();
    expect(screen.getByText("SEV")).toBeInTheDocument();
    expect(screen.getByText("MESSAGE")).toBeInTheDocument();
  });

  it("renders entry messages + component + timestampDisplay", () => {
    render(
      <EntryGrid
        entries={[
          {
            id: 1,
            lineNumber: 42,
            timestamp: 1776872905000,
            timestampDisplay: "2026-04-23 10:28:25",
            severity: "Warning",
            component: "Uploader",
            message: "retry after 5s",
            thread: undefined,
            threadDisplay: undefined,
            sourceFile: undefined,
            format: "Plain",
            filePath: "f",
            timezoneOffset: undefined,
          },
        ]}
      />
    );
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("2026-04-23 10:28:25")).toBeInTheDocument();
    expect(screen.getByText("Uploader")).toBeInTheDocument();
    expect(screen.getByText("WARN")).toBeInTheDocument();
    expect(screen.getByText("retry after 5s")).toBeInTheDocument();
  });
});
