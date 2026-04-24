import { describe, it, expect } from "vitest";
import { theme } from "./theme";

describe("theme tokens", () => {
  it("exposes the dark surface palette", () => {
    expect(theme.bg).toBe("#0b0f14");
    expect(theme.surface).toBe("#11161d");
    expect(theme.border).toBe("#1f2a36");
    expect(theme.accent).toBe("#5ee3c5");
  });

  it("provides a pill entry for each parse_state", () => {
    const states = ["ok", "okFallbacks", "partial", "failed", "pending", "stale"];
    for (const s of states) {
      expect(theme.pill[s as keyof typeof theme.pill]).toMatchObject({
        fg: expect.stringMatching(/^#[0-9a-f]{6}$/i),
        bg: expect.stringMatching(/^#[0-9a-f]{6}$/i),
        dot: expect.stringMatching(/^#[0-9a-f]{6}$/i),
      });
    }
  });

  it("exposes a dotted-pattern background string", () => {
    expect(theme.pattern.dots).toContain("radial-gradient");
    expect(theme.pattern.dots).toContain("12px");
  });
});
