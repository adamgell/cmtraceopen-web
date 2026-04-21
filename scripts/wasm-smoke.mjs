// WASM parser regression canary.
//
// Loads the Node-target wasm-pack output (pkg-node/), feeds it a real text
// fixture from the cmtraceopen submodule, and asserts per-entry field-level
// detail plus overall metadata match the known-good values pinned in the
// parser crate's own regression tests. If the WASM parser ever drifts from
// the desktop-side Rust parser's behavior, this canary fails the build.
//
// Run via `pnpm wasm:smoke` (which presumes `pnpm wasm:build:node` ran first).
//
// Vanilla Node, no test framework. Exits 0 on success, 1 on any mismatch.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

import { parseContent } from "../pkg-node/cmtrace_wasm.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");

// Fixture pinned in the cmtraceopen submodule. Source of truth for the
// expected values is `cmtraceopen/src-tauri/tests/parser_expanded_corpus.rs`,
// test `ccm_clean_fixture_detects_and_parses` (around line 89) — which
// asserts entries.len()==3, parse_errors==0, entry[0].component=="CcmExec",
// entry[1].component=="PolicyAgent", entry[2].severity=="Error" on this
// same fixture. We extend with thread / severity / timestamp coverage so
// field-level regressions trip CI too.
const FIXTURE_REL = "cmtraceopen/src-tauri/tests/corpus/ccm/clean/basic.log";
const EXPECTED_FORMAT = "Ccm";
const EXPECTED_TOTAL_LINES = 3;
const EXPECTED_PARSE_ERRORS = 0;

// Per-entry expectations, derived from the fixture content itself (3 CCM
// records emitted in order). component / thread are the CCM attribute
// values; severity maps from CCM `type` (1=Info, 2=Warning, 3=Error).
// matches parser_expanded_corpus.rs ccm_clean_fixture_detects_and_parses:
//   entries[0].component = "CcmExec", entries[1].component = "PolicyAgent",
//   entries[2].severity  = "Error".
const EXPECTED_ENTRIES = [
  { severity: "Info", component: "CcmExec", thread: 100 },
  { severity: "Info", component: "PolicyAgent", thread: 101 },
  { severity: "Error", component: "CcmExec", thread: 100 },
];

const fixturePath = resolve(repoRoot, FIXTURE_REL);

let content;
try {
  content = readFileSync(fixturePath, "utf8");
} catch (err) {
  console.error(
    `wasm-smoke: failed to read fixture at ${fixturePath}\n` +
      `  did you run \`git submodule update --init\`?\n` +
      `  underlying error: ${err.message}`,
  );
  process.exit(1);
}

let result;
try {
  result = parseContent(content, fixturePath, content.length);
} catch (err) {
  console.error(`wasm-smoke: parseContent threw: ${err?.message ?? err}`);
  process.exit(1);
}

const failures = [];

function expect(label, expected, actual) {
  // Strict equality for primitives; stringified comparison for reporting.
  if (expected !== actual) {
    failures.push(
      `  ${label}\n    expected: ${JSON.stringify(expected)}\n    actual:   ${JSON.stringify(actual)}`,
    );
  }
}

function assertEntry(index, expected) {
  const e = result?.entries?.[index];
  if (!e || typeof e !== "object") {
    failures.push(`  entry[${index}] missing or not an object (got ${JSON.stringify(e)})`);
    return;
  }
  expect(`entry[${index}].severity`, expected.severity, e.severity);
  expect(`entry[${index}].component`, expected.component, e.component);
  expect(`entry[${index}].thread`, expected.thread, e.thread);
  // timestamp (unix ms) must be present and numeric — any non-null finite
  // number is acceptable; specific wall-clock value depends on local TZ and
  // is covered more robustly by the Rust side.
  if (e.timestamp == null || typeof e.timestamp !== "number" || !Number.isFinite(e.timestamp)) {
    failures.push(
      `  entry[${index}].timestamp\n    expected: non-null finite number (unix ms)\n    actual:   ${JSON.stringify(e.timestamp)}`,
    );
  }
}

const actualCount = Array.isArray(result?.entries) ? result.entries.length : -1;
expect("entries.length", EXPECTED_ENTRIES.length, actualCount);
expect("formatDetected", EXPECTED_FORMAT, result?.formatDetected);
expect("totalLines", EXPECTED_TOTAL_LINES, result?.totalLines);
expect("parseErrors", EXPECTED_PARSE_ERRORS, result?.parseErrors);

for (let i = 0; i < EXPECTED_ENTRIES.length; i++) {
  assertEntry(i, EXPECTED_ENTRIES[i]);
}

if (failures.length > 0) {
  console.error(`wasm-smoke: ${failures.length} assertion(s) failed for ${FIXTURE_REL}`);
  for (const f of failures) console.error(f);
  console.error(
    "\nwasm-smoke: WASM parser output drifted from the desktop-side Rust parser.\n" +
      "  If this drift is intentional, update EXPECTED_* in scripts/wasm-smoke.mjs\n" +
      "  *after* confirming the parser crate's regression tests\n" +
      "  (parser_expanded_corpus.rs) also agree.",
  );
  process.exit(1);
}

console.log(
  `wasm-smoke: ok — ${EXPECTED_ENTRIES.length} entries asserted with field-level detail`,
);
