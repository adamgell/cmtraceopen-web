// Stubbed executor for the KQL bar. Returns a canned summary shape so the
// UI can render a plausible result strip without a real query compiler.
// Infers the rough table target from the first token (if present).
//
// TODO(real-executor): replace with a real compiler — see design spec
// open question "KQL executor boundary".

import type { FleetResultSummary } from "./bridge-state";
import { tokenize } from "./kql-lexer";

function pseudoNumber(query: string, mod: number): number {
  // Deterministic but varied so operators don't see perfectly repeated
  // numbers across different queries.
  let h = 0;
  for (let i = 0; i < query.length; i++) h = (h * 31 + query.charCodeAt(i)) | 0;
  return Math.abs(h) % mod;
}

export function runKqlStub(query: string): FleetResultSummary {
  const trimmed = query.trim();
  if (!trimmed) {
    return { matches: 0, devices: 0, sessions: 0, files: 0, groupBy: "device" };
  }
  const tokens = tokenize(trimmed);
  const firstTable = tokens.find((t) => t.kind === "table")?.text ?? "DeviceLog";
  const groupBy =
    firstTable === "Entry" ? "entry" :
    firstTable === "File" ? "file" :
    "device";
  const matches = 10 + pseudoNumber(trimmed, 90);
  return {
    matches,
    devices: 1 + pseudoNumber(trimmed, 12),
    sessions: 1 + pseudoNumber(trimmed + "s", 50),
    files: 1 + pseudoNumber(trimmed + "f", 25),
    groupBy,
  };
}
