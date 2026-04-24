import { describe, it, expect } from "vitest";
import { deriveHealth } from "./health-dot";

function now() { return new Date("2026-04-24T00:00:00Z").getTime(); }

describe("deriveHealth", () => {
  it("returns stale when lastSeen is > 24h old regardless of parse_state", () => {
    expect(deriveHealth({ parseState: "ok", lastSeenMs: now() - 25 * 3600 * 1000 }, now())).toBe("stale");
    expect(deriveHealth({ parseState: "failed", lastSeenMs: now() - 48 * 3600 * 1000 }, now())).toBe("stale");
  });

  it("maps fresh parse states to their own color", () => {
    const fresh = now() - 5 * 60 * 1000;
    expect(deriveHealth({ parseState: "ok", lastSeenMs: fresh }, now())).toBe("ok");
    expect(deriveHealth({ parseState: "ok-with-fallbacks", lastSeenMs: fresh }, now())).toBe("okFallbacks");
    expect(deriveHealth({ parseState: "partial", lastSeenMs: fresh }, now())).toBe("partial");
    expect(deriveHealth({ parseState: "failed", lastSeenMs: fresh }, now())).toBe("failed");
    expect(deriveHealth({ parseState: "pending", lastSeenMs: fresh }, now())).toBe("pending");
  });

  it("returns pending when parseState is unknown but lastSeen is fresh", () => {
    expect(deriveHealth({ parseState: "mystery", lastSeenMs: now() - 60_000 }, now())).toBe("pending");
  });

  it("returns stale when lastSeen is null", () => {
    expect(deriveHealth({ parseState: "ok", lastSeenMs: null }, now())).toBe("stale");
  });
});
