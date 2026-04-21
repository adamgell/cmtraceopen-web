#!/usr/bin/env bash
# Thin query helper for the CMTrace Open api-server registry routes.
#
# Subcommands:
#   devices [endpoint]                         -> GET  /v1/devices
#   sessions <device-id> [endpoint]            -> GET  /v1/devices/{id}/sessions
#   session  <session-id> [endpoint]           -> GET  /v1/sessions/{id}
#
# Endpoint defaults to http://localhost:8080 if omitted. Output is JSON
# (pretty-printed via jq) so you can pipe it further.

set -euo pipefail

DEFAULT_ENDPOINT="http://localhost:8080"

usage() {
  cat <<EOF
Usage: $0 <command> [args]

Commands:
  devices [endpoint]
      List devices known to the server.
  sessions <device-id> [endpoint]
      List sessions for a device.
  session <session-id> [endpoint]
      Fetch a single session.

Endpoint defaults to ${DEFAULT_ENDPOINT}.

Examples:
  $0 devices
  $0 devices http://192.168.2.50:8080
  $0 sessions WIN-LAB01
  $0 session 01900000-0000-7000-8000-000000000001
EOF
}

for bin in curl jq; do
  command -v "${bin}" >/dev/null 2>&1 || {
    echo "error: required binary not on PATH: ${bin}" >&2
    exit 2
  }
done

get_json() {
  local url="$1"
  local body
  body="$(mktemp)"
  # shellcheck disable=SC2064  # intentional early expansion of $body
  trap "rm -f '${body}'" RETURN
  local code
  code=$(curl -sS -o "${body}" -w '%{http_code}' "${url}")
  if [[ "${code}" != "200" ]]; then
    echo "error: GET ${url} -> ${code}" >&2
    if [[ -s "${body}" ]]; then cat "${body}" >&2; echo >&2; fi
    return 1
  fi
  jq . < "${body}"
}

cmd="${1:-}"
case "${cmd}" in
  devices)
    endpoint="${2:-${DEFAULT_ENDPOINT}}"
    get_json "${endpoint}/v1/devices"
    ;;
  sessions)
    if [[ $# -lt 2 ]]; then
      echo "error: sessions requires <device-id>" >&2
      usage >&2
      exit 2
    fi
    device="$2"
    endpoint="${3:-${DEFAULT_ENDPOINT}}"
    # URL-encode the device id minimally: jq's @uri handles the common cases
    # (spaces, slashes, unicode). Anything exotic in a device id is probably
    # a bug on the caller's end anyway.
    encoded=$(jq -rn --arg v "${device}" '$v | @uri')
    get_json "${endpoint}/v1/devices/${encoded}/sessions"
    ;;
  session)
    if [[ $# -lt 2 ]]; then
      echo "error: session requires <session-id>" >&2
      usage >&2
      exit 2
    fi
    session="$2"
    endpoint="${3:-${DEFAULT_ENDPOINT}}"
    get_json "${endpoint}/v1/sessions/${session}"
    ;;
  -h|--help|help|"")
    usage
    ;;
  *)
    echo "error: unknown command '${cmd}'" >&2
    usage >&2
    exit 2
    ;;
esac
