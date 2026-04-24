import { describe, it, expect } from "vitest";
import { tokenize } from "./kql-lexer";

describe("tokenize", () => {
  it("classifies table, pipe, keyword, field, operator, and string", () => {
    const tokens = tokenize('DeviceLog | where parse_state == "failed"');
    const classes = tokens.map((t) => t.kind);
    expect(classes).toEqual([
      "table", "whitespace",
      "pipe", "whitespace",
      "keyword", "whitespace",
      "field", "whitespace",
      "operator", "whitespace",
      "string",
    ]);
  });

  it("classifies function calls with numeric duration args", () => {
    const tokens = tokenize("ingested_utc > ago(24h)");
    const fn = tokens.find((t) => t.kind === "function");
    const num = tokens.find((t) => t.kind === "number");
    expect(fn?.text).toBe("ago");
    expect(num?.text).toBe("24h");
  });

  it("falls back to ident for unknown identifiers", () => {
    const tokens = tokenize("unknown_thing");
    expect(tokens[0]?.kind).toBe("ident");
  });

  it("preserves source spans", () => {
    const tokens = tokenize('DeviceLog | where x == 1');
    for (const t of tokens) {
      expect(t.end).toBeGreaterThanOrEqual(t.start);
    }
    // The sum of spans should equal the original length.
    expect(tokens[tokens.length - 1]?.end).toBe('DeviceLog | where x == 1'.length);
  });
});
