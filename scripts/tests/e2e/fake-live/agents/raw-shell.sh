#!/usr/bin/env bash
# fake-live/agents/raw-shell.sh — minimal shell marker used by fake-live tests

set -euo pipefail

PROFILE="${1:-default}"
printf 'raw-shell profile=%s\n' "$PROFILE"
sleep 1
