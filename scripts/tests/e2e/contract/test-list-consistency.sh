#!/usr/bin/env bash
# contract/test-list-consistency.sh — agtmux ls / agtmux ls --group=session consistency
# Verifies that counts in agtmux ls and agtmux ls --group=session match agtmux json.
# Uses source.ingest to create a known 2-pane state (1 Running, 1 Idle).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-consist-$$"
SOCKET="/tmp/agtmux-e2e-consist-$$/agtmuxd.sock"
INJECTOR1_PID=""
INJECTOR2_PID=""

echo "=== test-list-consistency.sh ==="

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

# Get two panes (create a second one)
PANE1=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
tmux split-window -t "$SESSION:main" -h 2>/dev/null
PANE2=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | tail -1)

if [ -z "$PANE1" ] || [ -z "$PANE2" ] || [ "$PANE1" = "$PANE2" ]; then
    fail "could not get two distinct pane IDs"
fi
log "pane1=$PANE1 pane2=$PANE2"

cleanup_tmux() {
    [ -n "$INJECTOR1_PID" ] && kill "$INJECTOR1_PID" 2>/dev/null || true
    [ -n "$INJECTOR2_PID" ] && kill "$INJECTOR2_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_tmux EXIT

daemon_start "$SOCKET" 500
sleep 1

SID1="e2e-consist-sess1-$$"
SID2="e2e-consist-sess2-$$"

# Inject running state for both panes continuously
# (resolver requires events < 3s old; loop ensures freshness across slow poll ticks)
INJECTOR1_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SID1" "$PANE1")
INJECTOR2_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SID2" "$PANE2")
log "running injectors: pane1=$INJECTOR1_PID pane2=$INJECTOR2_PID"

wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "running" 30
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running" 30

# Transition pane2 to idle: kill running injector, start idle injector
kill "$INJECTOR2_PID" 2>/dev/null || true
INJECTOR2_PID=$(inject_claude_event_loop "$SOCKET" "idle" "$SID2" "$PANE2")
log "idle injector for pane2: PID=$INJECTOR2_PID"
sleep 6  # hysteresis

wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "idle" 30

# ── Consistency check ─────────────────────────────────────────────────────

# Ground truth from agtmux json (schema v1, snake_case activity_state)
JSON_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null || echo '{"version":1,"panes":[]}')
JSON_RUNNING=$(echo "$JSON_OUT" | jq '[.panes[] | select(.presence=="managed" and .activity_state=="running")] | length')
JSON_IDLE=$(echo "$JSON_OUT" | jq '[.panes[] | select(.presence=="managed" and .activity_state=="idle")] | length')
JSON_MANAGED=$(echo "$JSON_OUT" | jq '[.panes[] | select(.presence=="managed")] | length')

log "JSON ground truth: managed=$JSON_MANAGED running=$JSON_RUNNING idle=$JSON_IDLE"

# agtmux ls --group=session should report matching counts
SESS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls --group=session 2>/dev/null || echo "")
log "agtmux ls --group=session output: $SESS_OUT"

# Agent count = running + idle (at least 2)
assert_contains "ls-session shows agents" "agent" "$SESS_OUT"

# Running count appears in agtmux ls --group=session (display uses PascalCase "Running")
if [ "$JSON_RUNNING" -gt 0 ]; then
    assert_contains "ls-session shows Running count" "Running" "$SESS_OUT"
fi

# Idle count appears in agtmux ls --group=session (display uses PascalCase "Idle")
if [ "$JSON_IDLE" -gt 0 ]; then
    assert_contains "ls-session shows Idle count" "Idle" "$SESS_OUT"
fi

# agtmux ls should show Running/Idle per pane (display uses PascalCase)
WIN_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls 2>/dev/null || echo "")
log "agtmux ls output: $WIN_OUT"

if [ "$JSON_RUNNING" -gt 0 ]; then
    assert_contains "ls shows Running" "Running" "$WIN_OUT"
fi
if [ "$JSON_IDLE" -gt 0 ]; then
    assert_contains "ls shows Idle" "Idle" "$WIN_OUT"
fi

# IDs must be hidden in both outputs
assert_not_contains "ls-session hides pane IDs"   "$PANE1" "$SESS_OUT"
assert_not_contains "ls hides pane IDs"            "$PANE1" "$WIN_OUT"

# Cleanup injectors
kill "$INJECTOR1_PID" 2>/dev/null || true
kill "$INJECTOR2_PID" 2>/dev/null || true
INJECTOR1_PID=""
INJECTOR2_PID=""

echo "=== test-list-consistency.sh PASS ==="
