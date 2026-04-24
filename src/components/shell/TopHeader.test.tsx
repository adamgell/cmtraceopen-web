import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { TopHeader } from "./TopHeader";

describe("TopHeader", () => {
  it("renders the brand + version", () => {
    render(<TopHeader onHelp={() => {}} version="1.2.3" />);
    expect(screen.getByText(/CMTRACE·OPEN/)).toBeInTheDocument();
    expect(screen.getByText(/v1\.2\.3/)).toBeInTheDocument();
  });

  it("fires onHelp when the Help button is clicked", () => {
    const onHelp = vi.fn();
    render(<TopHeader onHelp={onHelp} />);
    fireEvent.click(screen.getByRole("button", { name: /help/i }));
    expect(onHelp).toHaveBeenCalledOnce();
  });

  it("renders external nav links with target=_blank", () => {
    render(<TopHeader onHelp={() => {}} />);
    for (const label of ["Status", "Docs", "GitHub"]) {
      const link = screen.getByRole("link", { name: label });
      expect(link).toHaveAttribute("target", "_blank");
    }
  });
});
