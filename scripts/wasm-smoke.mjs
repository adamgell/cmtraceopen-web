// WASM parser regression canary.
//
// Loads the Node-target wasm-pack output (pkg-node/), feeds it real text
// fixtures from the cmtraceopen submodule, and asserts per-entry field-level
// detail plus overall metadata match the known-good values pinned in the
// parser crate's own regression tests. If the WASM parser ever drifts from
// the desktop-side Rust parser's behavior, this canary fails the build.
//
// Run via `pnpm wasm:smoke` (which presumes `pnpm wasm:build:node` ran first).
//
// Vanilla Node, no test framework. Exits 0 on success, 1 on any mismatch.
//
// Coverage:
//   * ccm/clean/basic.log     — full field-level assertions (primary canary)
//   * cbs/clean/CBS.log       — min-record count + format detection
//   * panther/clean/setupact.log — min-record count + format detection
//   * plain/unstructured.txt  — min-record count + plain fallback
//
// The CCM fixture remains the load-bearing assertion: exact entry count,
// severity, component, thread, and timestamp shape. The other three are
// smaller "didn't explode, format dispatched correctly" checks so new
// parsers catch breakage in the wasm bridge without claiming too much
// about the full field semantics (those are exhaustively tested by the
// Rust parser crate itself).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

import { parseContent } from "../pkg-node/cmtrace_wasm.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");

// --- Primary CCM canary -----------------------------------------------------
// Fixture pinned in the cmtraceopen submodule. Source of truth for the
// expected values is `cmtraceopen/src-tauri/tests/parser_expanded_corpus.rs`,
// test `ccm_clean_fixture_detects_and_parses` (around line 89) — which
// asserts entries.len()==3, parse_errors==0, entry[0].component=="CcmExec",
// entry[1].component=="PolicyAgent", entry[2].severity=="Error" on this
// same fixture. We extend with thread / severity / timestamp coverage so
// field-level regressions trip CI too.
const CCM_FIXTURE_REL = "cmtraceopen/src-tauri/tests/corpus/ccm/clean/basic.log";
const CCM_EXPECTED_FORMAT = "Ccm";
const CCM_EXPECTED_TOTAL_LINES = 3;
const CCM_EXPECTED_PARSE_ERRORS = 0;

// Per-entry expectations, derived from the fixture content itself (3 CCM
// records emitted in order). component / thread are the CCM attribute
// values; severity maps from CCM `type` (1=Info, 2=Warning, 3=Error).
// matches parser_expanded_corpus.rs ccm_clean_fixture_detects_and_parses:
//   entries[0].component = "CcmExec", entries[1].component = "PolicyAgent",
//   entries[2].severity  = "Error".
const CCM_EXPECTED_ENTRIES = [
  { severity: "Info", component: "CcmExec", thread: 100 },
  { severity: "Info", component: "PolicyAgent", thread: 101 },
  { severity: "Error", component: "CcmExec", thread: 100 },
];

// --- Secondary smoke fixtures ----------------------------------------------
// Lightweight shape checks across the other parser families. For each we
// assert:
//   * parseContent returned without throwing,
//   * formatDetected matches the expected LogFormat enum variant name,
//   * entries.length >= MIN_ENTRIES (we don't pin exact counts here because
//     the Rust crate's own regression tests do that more thoroughly;
//     instead we're guarding against "wasm build wired up the parser
//     but it silently drops all records").
//
// LogFormat naming note: CBS and Panther both resolve to
// `ParserImplementation::GenericTimestamped` and therefore surface as
// `formatDetected = "Timestamped"` in the wire DTO. The dedicated parser
// identity lives on `parserSelection.parser` (camelCase: "cbs", "panther",
// "plain"). We assert both so any reshuffle of the LogFormat <-> ParserKind
// mapping in detect.rs::compatibility_format() shows up here.
const SECONDARY_FIXTURES = [
  {
    label: "cbs",
    rel: "cmtraceopen/src-tauri/tests/corpus/cbs/clean/CBS.log",
    expectedFormat: "Timestamped",
    expectedParser: "cbs",
    minEntries: 2,
  },
  {
    label: "panther",
    rel: "cmtraceopen/src-tauri/tests/corpus/panther/clean/setupact.log",
    expectedFormat: "Timestamped",
    expectedParser: "panther",
    minEntries: 2,
  },
  {
    label: "plain",
    rel: "cmtraceopen/src-tauri/tests/corpus/plain/unstructured.txt",
    expectedFormat: "Plain",
    expectedParser: "plain",
    minEntries: 3,
  },
];

const failures = [];

function expect(label, expected, actual) {
  // Strict equality for primitives; stringified comparison for reporting.
  if (expected !== actual) {
    failures.push(
      `  ${label}\n    expected: ${JSON.stringify(expected)}\n    actual:   ${JSON.stringify(actual)}`,
    );
  }
}

function readFixture(rel) {
  const abs = resolve(repoRoot, rel);
  try {
    return { content: readFileSync(abs, "utf8"), abs };
  } catch (err) {
    console.error(
      `wasm-smoke: failed to read fixture at ${abs}\n` +
        `  did you run \`git submodule update --init\`?\n` +
        `  underlying error: ${err.message}`,
    );
    process.exit(1);
  }
}

function parseOrDie(content, filePath) {
  try {
    return parseContent(content, filePath, content.length);
  } catch (err) {
    console.error(`wasm-smoke: parseContent threw for ${filePath}: ${err?.message ?? err}`);
    process.exit(1);
  }
}

// --- Primary assertions: CCM ------------------------------------------------
{
  const { content, abs } = readFixture(CCM_FIXTURE_REL);
  const result = parseOrDie(content, abs);

  function assertEntry(index, expected) {
    const e = result?.entries?.[index];
    if (!e || typeof e !== "object") {
      failures.push(`  ccm entry[${index}] missing or not an object (got ${JSON.stringify(e)})`);
      return;
    }
    expect(`ccm entry[${index}].severity`, expected.severity, e.severity);
    expect(`ccm entry[${index}].component`, expected.component, e.component);
    expect(`ccm entry[${index}].thread`, expected.thread, e.thread);
    // timestamp (unix ms) must be present and numeric — any non-null finite
    // number is acceptable; specific wall-clock value depends on local TZ and
    // is covered more robustly by the Rust side.
    if (e.timestamp == null || typeof e.timestamp !== "number" || !Number.isFinite(e.timestamp)) {
      failures.push(
        `  ccm entry[${index}].timestamp\n    expected: non-null finite number (unix ms)\n    actual:   ${JSON.stringify(e.timestamp)}`,
      );
    }
  }

  const actualCount = Array.isArray(result?.entries) ? result.entries.length : -1;
  expect("ccm entries.length", CCM_EXPECTED_ENTRIES.length, actualCount);
  expect("ccm formatDetected", CCM_EXPECTED_FORMAT, result?.formatDetected);
  expect("ccm totalLines", CCM_EXPECTED_TOTAL_LINES, result?.totalLines);
  expect("ccm parseErrors", CCM_EXPECTED_PARSE_ERRORS, result?.parseErrors);

  for (let i = 0; i < CCM_EXPECTED_ENTRIES.length; i++) {
    assertEntry(i, CCM_EXPECTED_ENTRIES[i]);
  }
}

// --- Secondary smoke: CBS, Panther, plain ----------------------------------
for (const fx of SECONDARY_FIXTURES) {
  const { content, abs } = readFixture(fx.rel);
  const result = parseOrDie(content, abs);
  const actualCount = Array.isArray(result?.entries) ? result.entries.length : -1;

  expect(`${fx.label} formatDetected`, fx.expectedFormat, result?.formatDetected);
  expect(
    `${fx.label} parserSelection.parser`,
    fx.expectedParser,
    result?.parserSelection?.parser,
  );

  if (actualCount < fx.minEntries) {
    failures.push(
      `  ${fx.label} entries.length\n    expected: >= ${fx.minEntries}\n    actual:   ${actualCount}`,
    );
  }
}

if (failures.length > 0) {
  console.error(`wasm-smoke: ${failures.length} assertion(s) failed`);
  for (const f of failures) console.error(f);
  console.error(
    "\nwasm-smoke: WASM parser output drifted from the desktop-side Rust parser.\n" +
      "  If this drift is intentional, update EXPECTED_* / SECONDARY_FIXTURES\n" +
      "  in scripts/wasm-smoke.mjs *after* confirming the parser crate's\n" +
      "  regression tests (parser_expanded_corpus.rs) also agree.",
  );
  process.exit(1);
}

console.log(
  `wasm-smoke: ok — ccm field-level asserted (${CCM_EXPECTED_ENTRIES.length} entries) + ${SECONDARY_FIXTURES.length} secondary format checks`,
);
