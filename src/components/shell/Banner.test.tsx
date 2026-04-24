import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Banner } from "./Banner";

describe("Banner", () => {
  it("renders the empty-state kicker when no device is selected", () => {
    render(<Banner device={null} />);
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.queryByTestId("banner-chips")).not.toBeInTheDocument();
  });

  it("renders hostname, chips, and the kbd strip when a device is selected", () => {
    render(
      <Banner
        device={{
          deviceId: "GELL-01AA310",
          lastSeenLabel: "11h",
          sessionCount: 44,
          fileCount: 15,
          parseState: "ok-with-fallbacks",
        }}
      />
    );
    expect(screen.getByText("GELL-01AA310")).toBeInTheDocument();
    expect(screen.getByText("LAST SEEN")).toBeInTheDocument();
    expect(screen.getByText("11h")).toBeInTheDocument();
    expect(screen.getByText("SESSIONS")).toBeInTheDocument();
    expect(screen.getByText("44")).toBeInTheDocument();
    expect(screen.getByText(/focus query/)).toBeInTheDocument();
    expect(screen.getByText(/rail/)).toBeInTheDocument();
  });
});
