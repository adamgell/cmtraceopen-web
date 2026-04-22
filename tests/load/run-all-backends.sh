#!/usr/bin/env bash
# tests/load/run-all-backends.sh
#
# Run both k6 load-test scenarios against all four backend combinations and
# save the raw output to timestamped files.
#
# Prerequisites:
#   - k6 installed and on PATH
#   - Docker Compose stack available at repo root
#   - jq installed (used to pretty-print backend labels in output)
#
# Usage (from repo root):
#   bash tests/load/run-all-backends.sh
#
# Override the API base URL or scenario durations:
#   BASE_URL=http://bigmac26.local:8080 \
#   DURATION=5m \
#   bash tests/load/run-all-backends.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LOAD_DIR="${REPO_ROOT}/tests/load"
RESULTS_DIR="${REPO_ROOT}/tests/load/results"
BASE_URL="${BASE_URL:-http://localhost:8080}"
DURATION="${DURATION:-5m}"
DEVICES="${DEVICES:-100}"
OPERATORS="${OPERATORS:-10}"

mkdir -p "${RESULTS_DIR}"

RUN_TS="$(date -u +%Y%m%dT%H%M%SZ)"

# ---------------------------------------------------------------------------
# Backend matrix
# Each entry is a label plus the environment variable overrides needed to
# switch the Compose stack to that backend.  Adjust the env vars to match
# your api-server build when Azure Blob and Postgres backends land.
# ---------------------------------------------------------------------------

declare -a BACKEND_LABELS=(
  "sqlite-localfs"
  "postgres-localfs"
  "sqlite-azure"
  "postgres-azure"
)

declare -A BACKEND_ENV=(
  ["sqlite-localfs"]="CMTRACE_DATABASE_URL= CMTRACE_BLOB_STORE=local"
  ["postgres-localfs"]="CMTRACE_DATABASE_URL=postgresql://cmtrace:cmtrace@localhost:5432/cmtrace CMTRACE_BLOB_STORE=local"
  ["sqlite-azure"]="CMTRACE_DATABASE_URL= CMTRACE_BLOB_STORE=azure"
  ["postgres-azure"]="CMTRACE_DATABASE_URL=postgresql://cmtrace:cmtrace@localhost:5432/cmtrace CMTRACE_BLOB_STORE=azure"
)

run_scenario() {
  local label="$1"
  local script="$2"
  local scenario_name
  scenario_name="$(basename "${script}" .js)"
  local out="${RESULTS_DIR}/${RUN_TS}_${label}_${scenario_name}.txt"

  echo ""
  echo "======================================================="
  echo " Backend : ${label}"
  echo " Scenario: ${scenario_name}"
  echo " Output  : ${out}"
  echo "======================================================="

  # shellcheck disable=SC2086
  env ${BACKEND_ENV[${label}]} \
    BASE_URL="${BASE_URL}" \
    DURATION="${DURATION}" \
    DEVICES="${DEVICES}" \
    OPERATORS="${OPERATORS}" \
    k6 run "${script}" 2>&1 | tee "${out}"
}

restart_stack_with_env() {
  local label="$1"
  echo ""
  echo "--- Restarting Compose stack for backend: ${label} ---"
  # Export the backend-specific vars so docker compose picks them up.
  # shellcheck disable=SC2086
  export ${BACKEND_ENV[${label}]}
  (cd "${REPO_ROOT}" && docker compose down --volumes --remove-orphans && \
    docker compose up --build -d)
  # Wait for the api-server to be healthy.
  local retries=30
  until curl -sf "${BASE_URL}/healthz" > /dev/null 2>&1; do
    retries=$((retries - 1))
    if [ "${retries}" -eq 0 ]; then
      echo "ERROR: api-server did not become healthy in time." >&2
      exit 1
    fi
    sleep 2
  done
  echo "api-server is healthy."
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

for label in "${BACKEND_LABELS[@]}"; do
  restart_stack_with_env "${label}"

  # 1. Ingest scenario (seeds data used by the query scenario)
  run_scenario "${label}" "${LOAD_DIR}/k6-bundle-ingest.js"

  # 2. Query scenario (reads sessions seeded above)
  run_scenario "${label}" "${LOAD_DIR}/k6-query-mix.js"
done

echo ""
echo "All runs complete. Results saved to: ${RESULTS_DIR}/"
echo "Copy the numbers into docs/wave4/17-capacity-results.md."
