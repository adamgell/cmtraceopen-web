#!/usr/bin/env bash
# Ansible-free path: scp a plist to BigMac26 and (re)load it via launchctl.
# Use this when the other project can't assume Ansible is installed.
#
# Usage:
#   ./shell-only-deploy.sh path/to/one.gell.myservice.plist
#
# The plist filename stem MUST match its <key>Label</key> string.

set -euo pipefail

PLIST="${1:?usage: $0 path/to/<label>.plist}"
HOST="${MAC_HOST:-Adam.Gell@192.168.2.50}"
LABEL="$(basename "$PLIST" .plist)"
REMOTE_DIR="~/Library/LaunchAgents"
REMOTE_PATH="$REMOTE_DIR/$LABEL.plist"

echo ">> copying $PLIST to $HOST:$REMOTE_PATH"
ssh "$HOST" "mkdir -p $REMOTE_DIR"
scp "$PLIST" "$HOST:$REMOTE_PATH"

echo ">> reloading $LABEL"
ssh "$HOST" "
  launchctl unload -w $REMOTE_PATH 2>/dev/null || true
  launchctl load   -w $REMOTE_PATH
  launchctl list | grep -F '$LABEL' || { echo 'FAILED: agent did not load'; exit 1; }
"
echo ">> OK: $LABEL is loaded"
