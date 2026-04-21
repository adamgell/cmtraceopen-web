// WASM parser regression canary.
//
// Loads the Node-target wasm-pack output (pkg-node/), feeds it a real text
// fixture from the cmtraceopen submodule, and asserts the parsed entry count
// and detected format match the known-good values pinned in the parser crate's
// own regression tests. If the WASM parser ever drifts from the desktop-side
// Rust parser's behavior, this canary fails the build.
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
// expected entry count is `cmtraceopen/src-tauri/tests/parser_expanded_corpus.rs`,
// test `ccm_clean_fixture_detects_and_parses` — asserts `parsed.entries.len() == 3`
// on this same file.
const FIXTURE_REL = "cmtraceopen/src-tauri/tests/corpus/ccm/clean/basic.log";
const EXPECTED_ENTRY_COUNT = 3;
const EXPECTED_FORMAT = "Ccm";

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

const actualCount = Array.isArray(result?.entries) ? result.entries.length : -1;
const actualFormat = result?.formatDetected;

let failed = false;

if (actualCount !== EXPECTED_ENTRY_COUNT) {
  console.error(
    `wasm-smoke: entry count mismatch for ${FIXTURE_REL}\n` +
      `  expected: ${EXPECTED_ENTRY_COUNT}\n` +
      `  actual:   ${actualCount}`,
  );
  failed = true;
}

if (actualFormat !== EXPECTED_FORMAT) {
  console.error(
    `wasm-smoke: formatDetected mismatch for ${FIXTURE_REL}\n` +
      `  expected: ${JSON.stringify(EXPECTED_FORMAT)}\n` +
      `  actual:   ${JSON.stringify(actualFormat)}`,
  );
  failed = true;
}

if (failed) {
  console.error(
    "wasm-smoke: WASM parser output drifted from the desktop-side Rust parser.\n" +
      "  If this drift is intentional, update EXPECTED_ENTRY_COUNT / EXPECTED_FORMAT\n" +
      "  in scripts/wasm-smoke.mjs *after* confirming the parser crate's regression\n" +
      "  tests (parser_expanded_corpus.rs / parser_regression_corpus.rs) also agree.",
  );
  process.exit(1);
}

console.log(
  `wasm-smoke: ok — ${FIXTURE_REL} parsed into ${actualCount} entries ` +
    `(formatDetected=${JSON.stringify(actualFormat)})`,
);
