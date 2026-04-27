import { describe, it, expect } from "vitest";
import { suggest } from "./kql-autocomplete";

describe("kql-autocomplete", () => {
  it("suggests tables when query is empty", () => {
    const items = suggest("", 0);
    const labels = items.map((s) => s.label);
    expect(labels).toContain("DeviceLog");
    expect(labels).toContain("File");
    expect(labels).toContain("Entry");
  });

  it("suggests keywords after pipe", () => {
    const q = "DeviceLog | ";
    const items = suggest(q, q.length);
    const labels = items.map((s) => s.label);
    expect(labels).toContain("where");
    expect(labels).toContain("summarize");
    expect(labels).toContain("project");
  });

  it("suggests fields after 'where'", () => {
    const q = "DeviceLog | where ";
    const items = suggest(q, q.length);
    const labels = items.map((s) => s.label);
    expect(labels).toContain("device_id");
    expect(labels).toContain("parse_state");
    expect(labels).toContain("ingested_utc");
  });

  it("suggests operators after a field name", () => {
    const q = "DeviceLog | where parse_state ";
    const items = suggest(q, q.length);
    const labels = items.map((s) => s.label);
    expect(labels).toContain("==");
    expect(labels).toContain("!=");
    expect(labels).toContain("has");
    expect(labels).toContain("contains");
  });

  it("suggests example values after an operator", () => {
    const q = "DeviceLog | where parse_state == ";
    const items = suggest(q, q.length);
    const labels = items.map((s) => s.label);
    expect(labels).toContain('"ok"');
    expect(labels).toContain('"failed"');
  });

  it("filters suggestions by partial input", () => {
    const q = "Dev";
    const items = suggest(q, q.length);
    expect(items.length).toBe(1);
    expect(items[0]!.label).toBe("DeviceLog");
  });

  it("uses correct table context for field suggestions", () => {
    const q = "Entry | where ";
    const items = suggest(q, q.length);
    const labels = items.map((s) => s.label);
    expect(labels).toContain("severity");
    expect(labels).toContain("message");
    expect(labels).not.toContain("device_id");
  });
});
