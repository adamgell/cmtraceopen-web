import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { RowDetail } from "./RowDetail";
import type { LogEntry } from "../../lib/log-types";

const entry: LogEntry = {
  id: 1,
  lineNumber: 42,
  timestamp: 1776872905000,
  timestampDisplay: "2026-04-23 10:28:25",
  severity: "Info",
  component: "Uploader",
  message: "bundle finalized",
  thread: undefined,
  threadDisplay: undefined,
  sourceFile: undefined,
  format: "Plain",
  filePath: "f",
  timezoneOffset: undefined,
};

describe("RowDetail", () => {
  it("renders null when no entry is provided", () => {
    const { container } = render(<RowDetail entry={null} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("shows the message + metadata when an entry is provided", () => {
    render(<RowDetail entry={entry} onClose={() => {}} />);
    expect(screen.getByText("bundle finalized")).toBeInTheDocument();
    expect(screen.getByText(/42/)).toBeInTheDocument();
    expect(screen.getByText(/Uploader/)).toBeInTheDocument();
  });

  it("calls onClose when the close button is clicked", () => {
    const onClose = vi.fn();
    render(<RowDetail entry={entry} onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
