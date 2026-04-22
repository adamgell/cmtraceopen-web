#!/usr/bin/env bash
# tools/agent-redact-test.sh
#
# Operator preview tool: pipe a sample file (or use a fixture path) through
# the agent's default PII-redaction ruleset and print the diff side-by-side.
#
# Usage:
#   ./tools/agent-redact-test.sh <file>               # preview single file
#   cat /path/to/ccmexec.log | ./tools/agent-redact-test.sh -  # stdin
#   ./tools/agent-redact-test.sh --fixture             # run against built-in fixture
#
# The script builds a small Rust helper binary inline using 'cargo script'
# (or, if that is unavailable, falls back to writing a temporary Cargo project
# and building it with cargo). The binary reuses the same regex rules baked
# into the agent so there is no drift between preview and production.
#
# Requirements:
#   - cargo (Rust toolchain on PATH)
#   - diff / colordiff (for the side-by-side view; colordiff is optional)
#
# Exit codes:
#   0  success (redacted output written to stdout)
#   1  usage error
#   2  build error

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
AGENT_CRATE="${REPO_ROOT}/crates/agent"

# ─── Helpers ──────────────────────────────────────────────────────────────────

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "required tool '$1' not found on PATH"; }

need cargo

# ─── Argument handling ────────────────────────────────────────────────────────

FIXTURE_MODE=false
INPUT_FILE=""

case "${1:-}" in
    --fixture)
        FIXTURE_MODE=true
        ;;
    -)
        INPUT_FILE="-"
        ;;
    "")
        echo "Usage: $0 <file|-> [--fixture]"
        echo "       $0 --fixture"
        echo ""
        echo "  <file>     Path to a log file to redact"
        echo "  -          Read from stdin"
        echo "  --fixture  Use the built-in fixture containing all PII types"
        exit 1
        ;;
    *)
        INPUT_FILE="$1"
        [[ -f "${INPUT_FILE}" ]] || die "file not found: ${INPUT_FILE}"
        ;;
esac

# ─── Build the redact-preview binary ──────────────────────────────────────────

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# Write a minimal Cargo.toml that depends on the local agent crate.
cat >"${WORK_DIR}/Cargo.toml" <<'TOML'
[package]
name = "redact-preview"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "redact-preview"
path = "src/main.rs"

[dependencies]
agent = { path = "AGENT_CRATE_PATH" }
TOML
# Substitute the real path (sed avoids issues with special chars in the path).
sed -i "s|AGENT_CRATE_PATH|${AGENT_CRATE}|g" "${WORK_DIR}/Cargo.toml"

mkdir -p "${WORK_DIR}/src"
cat >"${WORK_DIR}/src/main.rs" <<'RUST'
//! Minimal stdin→stdout redaction preview tool.
//! Reads the input, applies the agent's default redaction rules, and prints
//! the result to stdout. Exit code 0 means success.

use std::io::{self, Read};
// The lib crate name is `cmtraceopen_agent` (see [lib] section in agent/Cargo.toml).
use cmtraceopen_agent::config::AgentConfig;
use cmtraceopen_agent::redact::Redactor;

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).expect("read stdin");

    let cfg = AgentConfig::default(); // enabled = true, default rules
    let redactor = Redactor::from_config(&cfg).expect("compile rules");
    let output = redactor.apply(&input);
    print!("{output}");
}
RUST

echo "Building redact-preview binary (first run may take a moment)…" >&2
if ! cargo build --manifest-path "${WORK_DIR}/Cargo.toml" --quiet 2>&1; then
    die "build failed — see output above"
fi

BINARY="${WORK_DIR}/target/debug/redact-preview"
[[ -x "${BINARY}" ]] || die "binary not found at ${BINARY}"

# ─── Fixture ──────────────────────────────────────────────────────────────────

if $FIXTURE_MODE; then
    FIXTURE_FILE="$(mktemp)"
    trap 'rm -f "${FIXTURE_FILE}"' EXIT

    cat >"${FIXTURE_FILE}" <<'FIXTURE'
-- CMTrace Open agent-redact-test.sh built-in fixture --
This file contains all PII types that the default ruleset covers.

username_path  : C:\Users\johndoe\AppData\Local\Temp\intune.log
guid           : EnrollmentID 550e8400-e29b-41d4-a716-446655440000 applied
email          : Admin alice@corp.example.com authorised policy
ipv4_internal  : MDM server at 10.20.30.40 responded 200 OK
public_ip      : NTP sync from 203.0.113.5 ok (should NOT be redacted)
FIXTURE

    INPUT_FILE="${FIXTURE_FILE}"
fi

# ─── Run the redactor and show diff ───────────────────────────────────────────

REDACTED_FILE="$(mktemp)"
trap 'rm -f "${REDACTED_FILE}"' EXIT

if [[ "${INPUT_FILE}" == "-" ]]; then
    cat | "${BINARY}" >"${REDACTED_FILE}"
else
    "${BINARY}" <"${INPUT_FILE}" >"${REDACTED_FILE}"
fi

echo ""
echo "=== Redacted output ==="
cat "${REDACTED_FILE}"
echo ""

if diff -q "${INPUT_FILE}" "${REDACTED_FILE}" >/dev/null 2>&1; then
    echo "=== No changes: input contained no PII matching the default ruleset ==="
else
    echo "=== Diff (original → redacted) ==="
    DIFF_CMD="diff"
    command -v colordiff >/dev/null 2>&1 && DIFF_CMD="colordiff"
    "${DIFF_CMD}" --unified "${INPUT_FILE}" "${REDACTED_FILE}" || true
fi
