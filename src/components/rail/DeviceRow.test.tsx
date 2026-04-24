// Tests for the per-device row in the left rail.
//
// DeviceRow has two rendering modes — collapsed (rail width 56px, icon + slug
// only) and expanded (rail width 220px, full deviceId + last-seen label). A
// click fires `onSelect(deviceId)` in either mode. The three tests below lock
// each of those three behaviors.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DeviceRow } from "./DeviceRow";

describe("DeviceRow", () => {
  const device = { deviceId: "GELL-01AA310", lastSeenLabel: "11h", health: "okFallbacks" as const };

  it("renders collapsed: dot + 4-char slug only", () => {
    render(<DeviceRow device={device} expanded={false} active={false} onSelect={() => {}} />);
    expect(screen.getByText("GELL")).toBeInTheDocument();
    expect(screen.queryByText("GELL-01AA310")).not.toBeInTheDocument();
  });

  it("renders expanded: full id + last-seen delta", () => {
    render(<DeviceRow device={device} expanded={true} active={false} onSelect={() => {}} />);
    expect(screen.getByText("GELL-01AA310")).toBeInTheDocument();
    expect(screen.getByText("11h")).toBeInTheDocument();
  });

  it("fires onSelect with the device id on click", () => {
    let captured = "";
    render(<DeviceRow device={device} expanded={false} active={false} onSelect={(id) => (captured = id)} />);
    fireEvent.click(screen.getByRole("button"));
    expect(captured).toBe("GELL-01AA310");
  });

  it("uppercases the slug for lowercase device ids", () => {
    render(
      <DeviceRow
        device={{ deviceId: "gell-01", lastSeenLabel: "5m", health: "ok" }}
        expanded={false}
        active={false}
        onSelect={() => {}}
      />
    );
    // "gell-01" → uppercased to "GELL-01" → [A-Z0-9] match → "GELL" (first 4 chars).
    expect(screen.getByText("GELL")).toBeInTheDocument();
  });

  it("pads short slugs with · so the column layout stays stable", () => {
    render(
      <DeviceRow
        device={{ deviceId: "X-1", lastSeenLabel: "5m", health: "ok" }}
        expanded={false}
        active={false}
        onSelect={() => {}}
      />
    );
    // "X-1" → "X1" → padded to "X1··" (4 chars, dot-filler).
    expect(screen.getByText("X1··")).toBeInTheDocument();
  });
});
