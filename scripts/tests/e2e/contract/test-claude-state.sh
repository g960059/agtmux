#!/usr/bin/env bash
# contract/test-claude-state.sh — Claude state transitions via source.ingest injection
# Verifies: tool_start → running (Deterministic), idle → idle transition.
# No real Claude CLI needed.
# activity_state values use snake_case (agtmux json schema v1).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-claude-$$"
SOCKET="/tmp/agtmux-e2e-claude-$$/agtmuxd.sock"
INJECTOR_PID=""

echo "=== test-claude-state.sh ==="

# ── Setup: isolated tmux session + daemon ─────────────────────────────────

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

# Get the pane ID from the new session
PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
if [ -z "$PANE_ID" ]; then
    fail "could not get pane_id from tmux session $SESSION"
fi
log "using pane_id=$PANE_ID session=$SESSION"

# Add tmux session cleanup to EXIT
cleanup_tmux() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_tmux EXIT

daemon_start "$SOCKET" 500
sleep 1  # Let daemon do its first poll

# ── Scenario 1: tool_start → running (Deterministic) ─────────────────────
# The resolver requires events < 3s old. Inject continuously in background
# so there is always a fresh event when the next poll tick processes it.

SESSION_ID="e2e-claude-sess-$$"
INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "tool_start injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 1: tool_start → running (Deterministic)"

# ── Scenario 2: idle → idle ───────────────────────────────────────────────
# Switch to idle injection; wait for hysteresis to settle.

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "idle" "$SESSION_ID" "$PANE_ID")
log "idle injector PID=$INJECTOR_PID"
sleep 6  # hysteresis: poll=500ms; use 6s for safety

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 2: idle → idle (Deterministic)"

# ── Scenario 3: tool_start again after idle ───────────────────────────────

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "tool_start (recovery) injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 20

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 3: tool_start recovers to running after idle"

echo "=== test-claude-state.sh PASS ==="
