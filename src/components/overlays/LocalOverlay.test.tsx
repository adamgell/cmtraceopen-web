import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// Stub LocalMode so the test is independent of the real component's deps
// (Fluent UI, WASM, etc.).
vi.mock("../LocalMode", () => ({
  __esModule: true,
  LocalMode: () => <div data-testid="local-mode">local-mode</div>,
}));

import { LocalOverlay } from "./LocalOverlay";

describe("LocalOverlay", () => {
  it("renders nothing when closed", () => {
    const { container } = render(<LocalOverlay open={false} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders LocalMode when open", () => {
    render(<LocalOverlay open={true} onClose={() => {}} />);
    expect(screen.getByTestId("local-mode")).toBeInTheDocument();
  });

  it("fires onClose on Esc", () => {
    const onClose = vi.fn();
    render(<LocalOverlay open={true} onClose={onClose} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
