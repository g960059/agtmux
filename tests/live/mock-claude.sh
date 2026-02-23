#!/usr/bin/env bash
# Mock Claude agent: cycles through 8 states, writing ground-truth timestamps.
set -euo pipefail

HOLD_SEC="${HOLD_SEC:-8}"
TIMESTAMP_FILE="${TIMESTAMP_FILE:?TIMESTAMP_FILE is required}"
PANE_ID="${PANE_ID:?PANE_ID is required}"

emit() {
    local token="$1"
    local state="$2"
    local ts
    ts="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    printf '\033[2J\033[H%s\n' "$token"
    printf '{"ts":"%s","state":"%s","pane_id":"%s"}\n' "$ts" "$state" "$PANE_ID" >> "$TIMESTAMP_FILE"
    sleep "$HOLD_SEC"
}

emit "running"          "Running"
emit "waiting_approval" "WaitingApproval"
emit "working"          "Running"
emit "waiting_input"    "WaitingInput"
emit "error"            "Error"
emit "idle"             "Idle"
emit "thinking"         "Unknown"
emit "completed"        "Idle"

echo "MOCK_COMPLETE"
