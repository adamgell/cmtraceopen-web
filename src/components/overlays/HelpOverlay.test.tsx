import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HelpOverlay } from "./HelpOverlay";

describe("HelpOverlay", () => {
  it("renders nothing when closed", () => {
    const { container } = render(<HelpOverlay open={false} onClose={() => {}} />);
    expect(container.firstChild).toBeNull();
  });

  it("lists the known shortcuts when open", () => {
    render(<HelpOverlay open={true} onClose={() => {}} />);
    expect(screen.getByText(/focus query/i)).toBeInTheDocument();
    expect(screen.getByText(/toggle rail/i)).toBeInTheDocument();
    expect(screen.getByText(/next file/i)).toBeInTheDocument();
  });

  it("fires onClose when the backdrop is clicked", () => {
    const onClose = vi.fn();
    render(<HelpOverlay open={true} onClose={onClose} />);
    fireEvent.click(screen.getByTestId("help-backdrop"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
