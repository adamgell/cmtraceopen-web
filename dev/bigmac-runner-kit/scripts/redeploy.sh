#!/usr/bin/env bash
#
# redeploy.sh — refresh the cmtraceopen-web deployment on BigMac26.
#
# Zero-ceremony wrapper around the SSH + git pull + docker compose dance we
# used to fire agents for. Runs from any cwd; resolves its own location to
# find the repo root and the SSH key committed alongside the kit.
#
# See ./README.md for usage, and ../README.md ("Two deploy paths") for how
# this relates to the Ansible kit.

set -euo pipefail

# ---------------------------------------------------------------------------
# Resolve paths (script is at <repo>/dev/bigmac-runner-kit/scripts/redeploy.sh
# so repo root is three levels up).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
KIT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SSH_KEY="$KIT_DIR/id_ed25519"

# ---------------------------------------------------------------------------
# Remote target
# ---------------------------------------------------------------------------
REMOTE_HOST="192.168.2.50"
REMOTE_USER="Adam.Gell"
REMOTE_REPO='$HOME/repo/cmtraceopen-web'
DOCKER_SOCK="unix:///Users/Adam.Gell/.colima/default/docker.sock"
HEALTH_URL="http://localhost:8080/healthz"
READY_URL="http://localhost:8080/readyz"
STATUS_URL="http://localhost:8080/"

# ---------------------------------------------------------------------------
# Defaults / flags
# ---------------------------------------------------------------------------
BRANCH="main"
SKIP_BUILD=0
NO_SMOKE=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [options]

Refresh the cmtraceopen-web deployment on BigMac26 (192.168.2.50).

Options:
  -b, --branch NAME     Branch to deploy (default: main)
  -s, --skip-build      docker compose up -d without --build (fast redeploy)
  -n, --no-smoke        Skip the post-deploy curl health checks
  -h, --help            Show this help and exit

Examples:
  $(basename "$0")                         # deploy main, full rebuild
  $(basename "$0") --branch feat/foo       # deploy a feature branch
  $(basename "$0") --skip-build            # fast restart, same image
EOF
}

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
  case "$1" in
    -b|--branch)
      [ $# -ge 2 ] || { echo "ERROR: --branch needs a value" >&2; exit 1; }
      BRANCH="$2"
      shift 2
      ;;
    -s|--skip-build)
      SKIP_BUILD=1
      shift
      ;;
    -n|--no-smoke)
      NO_SMOKE=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------
if [ ! -f "$SSH_KEY" ]; then
  echo "ERROR: SSH key not found at $SSH_KEY" >&2
  echo "       The kit expects an ed25519 private key at dev/bigmac-runner-kit/id_ed25519." >&2
  exit 2
fi

# Tighten perms if the filesystem supports it (no-op on Windows bind mounts).
chmod 600 "$SSH_KEY" 2>/dev/null || true

SSH_OPTS=(
  -i "$SSH_KEY"
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=10
  -o BatchMode=yes
)

echo "==> BigMac26 redeploy"
echo "    branch:     $BRANCH"
echo "    skip-build: $SKIP_BUILD"
echo "    no-smoke:   $NO_SMOKE"
echo "    repo root:  $REPO_ROOT"
echo "    ssh key:    $SSH_KEY"
echo

# Quick reachability probe — fail fast with exit 2 on SSH trouble.
if ! ssh "${SSH_OPTS[@]}" "$REMOTE_USER@$REMOTE_HOST" 'true' 2>/tmp/redeploy-ssh-err; then
  echo "ERROR: SSH to $REMOTE_USER@$REMOTE_HOST failed." >&2
  echo "       Key: $SSH_KEY" >&2
  if [ -s /tmp/redeploy-ssh-err ]; then
    echo "       Detail:" >&2
    sed 's/^/         /' /tmp/redeploy-ssh-err >&2
  fi
  rm -f /tmp/redeploy-ssh-err
  exit 2
fi
rm -f /tmp/redeploy-ssh-err

# ---------------------------------------------------------------------------
# Remote script — all brew-provided tools need shellenv on non-interactive SSH.
# ---------------------------------------------------------------------------
BUILD_FLAG="--build"
if [ "$SKIP_BUILD" = "1" ]; then
  BUILD_FLAG=""
fi

REMOTE_SCRIPT=$(cat <<REMOTE
set -euo pipefail
eval "\$(/opt/homebrew/bin/brew shellenv)"
export DOCKER_HOST="$DOCKER_SOCK"

cd $REMOTE_REPO

echo "--- git fetch origin"
git fetch origin

OLD_HEAD=\$(git rev-parse --short HEAD)

echo "--- checkout $BRANCH"
git checkout "$BRANCH"

echo "--- git pull --ff-only"
git pull --ff-only

echo "--- git submodule update --init --recursive"
git submodule update --init --recursive

NEW_HEAD=\$(git rev-parse --short HEAD)
echo "HEAD: \$OLD_HEAD -> \$NEW_HEAD"

echo "--- docker compose down (keeping volumes)"
docker compose down

echo "--- docker compose up -d $BUILD_FLAG"
if ! docker compose up -d $BUILD_FLAG; then
  echo "!!! docker compose up failed — dumping api-server logs (last 30)" >&2
  docker compose logs --tail=30 api-server >&2 || true
  exit 1
fi

echo "--- docker compose ps"
docker compose ps

echo "REDEPLOY_SUMMARY old=\$OLD_HEAD new=\$NEW_HEAD branch=$BRANCH"
REMOTE
)

echo "==> Running remote redeploy..."
ssh "${SSH_OPTS[@]}" "$REMOTE_USER@$REMOTE_HOST" "bash -s" <<<"$REMOTE_SCRIPT" | tee /tmp/redeploy-remote.out

SUMMARY_LINE=$(grep -E '^REDEPLOY_SUMMARY ' /tmp/redeploy-remote.out | tail -1 || true)
rm -f /tmp/redeploy-remote.out

# ---------------------------------------------------------------------------
# Smoke tests (run remotely so we hit the host loopback, not the LAN)
# ---------------------------------------------------------------------------
if [ "$NO_SMOKE" = "1" ]; then
  echo
  echo "==> Skipping smoke tests (--no-smoke)."
else
  echo
  echo "==> Smoke tests"
  SMOKE_SCRIPT=$(cat <<SMOKE
set -uo pipefail
eval "\$(/opt/homebrew/bin/brew shellenv)"

echo "--- GET $HEALTH_URL"
curl -sS "$HEALTH_URL" | jq . || echo "(healthz failed or not JSON)"

echo "--- GET $READY_URL"
curl -sS "$READY_URL" | jq . || echo "(readyz failed or not JSON)"

echo "--- status page uptime line"
curl -sS "$STATUS_URL" 2>/dev/null | grep Uptime || true
SMOKE
)
  ssh "${SSH_OPTS[@]}" "$REMOTE_USER@$REMOTE_HOST" "bash -s" <<<"$SMOKE_SCRIPT" || {
    echo "WARN: smoke test block exited non-zero" >&2
  }
fi

# ---------------------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------------------
echo
echo "==> Summary"
if [ -n "$SUMMARY_LINE" ]; then
  echo "    $SUMMARY_LINE"
else
  echo "    (no summary line captured — check output above)"
fi
echo "    Status page: $STATUS_URL"
echo "    Done."
