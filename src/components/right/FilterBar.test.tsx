import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterBar } from "./FilterBar";

describe("FilterBar", () => {
  it("toggles severity pills", () => {
    const onChange = vi.fn();
    render(
      <FilterBar
        filters={{ info: true, warn: true, error: true, search: "", component: "" }}
        totals={{ rendered: 500, total: 1_300_000 }}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /info/i }));
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ info: false, warn: true, error: true })
    );
  });

  it("shows the rendered / total counter", () => {
    render(
      <FilterBar
        filters={{ info: true, warn: true, error: true, search: "", component: "" }}
        totals={{ rendered: 500, total: 1_300_000 }}
        onChange={() => {}}
      />
    );
    expect(screen.getByText(/500 \/ 1\.3M/)).toBeInTheDocument();
  });
});
