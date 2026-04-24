import { describe, it, expect } from "vitest";
import { kqlSchema, tablesList, fieldsFor } from "./kql-schema";

describe("kql-schema", () => {
  it("exposes the three tables", () => {
    expect(tablesList()).toEqual(["DeviceLog", "File", "Entry"]);
  });

  it("fieldsFor returns the declared fields for each table", () => {
    expect(fieldsFor("DeviceLog").map((f) => f.name)).toContain("parse_state");
    expect(fieldsFor("File").map((f) => f.name)).toContain("relative_path");
    expect(fieldsFor("Entry").map((f) => f.name)).toContain("ts_ms");
  });

  it("fieldsFor returns empty for unknown tables", () => {
    expect(fieldsFor("Unknown")).toEqual([]);
  });

  it("every field has a type and at least one example", () => {
    for (const t of tablesList()) {
      for (const f of kqlSchema[t]) {
        expect(f.type.length).toBeGreaterThan(0);
        expect(f.examples.length).toBeGreaterThan(0);
      }
    }
  });
});
