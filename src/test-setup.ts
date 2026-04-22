// Vitest setup — runs once before each test file.
//
// Pulls in `@testing-library/jest-dom` so matchers like `toBeInTheDocument`,
// `toBeDisabled`, `toHaveTextContent` work against the jsdom-rendered tree.

import "@testing-library/jest-dom/vitest";

// jsdom does not compute layout — every element reports a 0×0 bounding rect,
// which makes `@tanstack/react-virtual` think the scroll container has zero
// visible area and renders no rows. Stub it with a sensible default so the
// virtualizer believes there is screen real estate available.
if (typeof window !== "undefined") {
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => ({
      x: 0,
      y: 0,
      width: 800,
      height: 600,
      top: 0,
      left: 0,
      right: 800,
      bottom: 600,
      toJSON: () => ({}),
    }),
  });

  // Same reason — these are read by the virtualizer's `measureElement` path.
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get() {
      return 600;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get() {
      return 800;
    },
  });

  // ResizeObserver is not implemented in jsdom; the virtualizer uses it.
  if (!("ResizeObserver" in window)) {
    (window as unknown as { ResizeObserver: unknown }).ResizeObserver =
      class ResizeObserver {
        observe() {}
        unobserve() {}
        disconnect() {}
      };
  }
}
