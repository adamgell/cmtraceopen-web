import { describe, it, expect } from "vitest";
import { runKqlStub } from "./kql-executor-stub";

describe("runKqlStub", () => {
  it("returns a plausible shape for a DeviceLog query", () => {
    const res = runKqlStub('DeviceLog | where parse_state == "failed"');
    expect(res.matches).toBeGreaterThanOrEqual(0);
    expect(res.devices).toBeGreaterThanOrEqual(0);
    expect(res.sessions).toBeGreaterThanOrEqual(0);
    expect(res.files).toBeGreaterThanOrEqual(0);
    expect(typeof res.groupBy).toBe("string");
  });

  it("infers groupBy = 'device' when the pipeline ends at DeviceLog", () => {
    const res = runKqlStub("DeviceLog | where parse_state == \"failed\"");
    expect(res.groupBy).toBe("device");
  });

  it("infers groupBy = 'file' when the query targets File", () => {
    const res = runKqlStub("File | where relative_path has \"ccmexec\"");
    expect(res.groupBy).toBe("file");
  });

  it("returns zero matches for an empty query", () => {
    const res = runKqlStub("");
    expect(res.matches).toBe(0);
  });
});
