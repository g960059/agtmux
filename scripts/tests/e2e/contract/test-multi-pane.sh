#!/usr/bin/env bash
# contract/test-multi-pane.sh — same-CWD multi-pane regression test (T-124)
# Verifies that when 2 panes share the same CWD, both get managed independently.
# Also tests multiple agents in the same tmux session with different session_ids.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-multipane-$$"
SOCKET="/tmp/agtmux-e2e-multipane-$$/agtmuxd.sock"
INJECTOR_A_PID=""
INJECTOR_B_PID=""

echo "=== test-multi-pane.sh ==="

# ── Setup: 3-pane session ─────────────────────────────────────────────────

tmux new-session -d -s "$SESSION" -n main 2>/dev/null
tmux split-window -t "$SESSION:main" -h 2>/dev/null
tmux split-window -t "$SESSION:main" -h 2>/dev/null

PANES=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null)
PANE1=$(echo "$PANES" | sed -n '1p')
PANE2=$(echo "$PANES" | sed -n '2p')
PANE3=$(echo "$PANES" | sed -n '3p')

if [ -z "$PANE1" ] || [ -z "$PANE2" ] || [ -z "$PANE3" ]; then
    fail "could not get 3 pane IDs (got: $PANES)"
fi
log "pane1=$PANE1 pane2=$PANE2 pane3=$PANE3"

cleanup_tmux() {
    [ -n "$INJECTOR_A_PID" ] && kill "$INJECTOR_A_PID" 2>/dev/null || true
    [ -n "$INJECTOR_B_PID" ] && kill "$INJECTOR_B_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_tmux EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario 1: Two panes, two different Claude sessions ──────────────────
# Each pane has its own session_id → both should become managed independently.
# Inject continuously (resolver requires < 3s old events).

SID_A="e2e-session-A-$$"
SID_B="e2e-session-B-$$"

INJECTOR_A_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SID_A" "$PANE1")
INJECTOR_B_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SID_B" "$PANE2")
log "injectors: A=$INJECTOR_A_PID B=$INJECTOR_B_PID"

wait_for_agtmux_state "$SOCKET" "$PANE1" "presence"       "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE2" "presence"       "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "running" 20
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running" 20

pass "Scenario 1: Two panes in same session managed independently"

# ── Scenario 2: Pane 3 stays unmanaged ───────────────────────────────────

PANE3_PRESENCE=$(jq_get "$SOCKET" "$PANE3" "presence")
if [ "$PANE3_PRESENCE" = "managed" ]; then
    fail "Scenario 2: pane3 should be unmanaged but got managed"
fi
pass "Scenario 2: Pane3 (no events) stays unmanaged"

# ── Scenario 3: Pane1 goes idle while pane2 stays running ────────────────

kill "$INJECTOR_A_PID" 2>/dev/null || true
INJECTOR_A_PID=$(inject_claude_event_loop "$SOCKET" "idle" "$SID_A" "$PANE1")
log "idle injector for pane1: PID=$INJECTOR_A_PID"
sleep 6  # hysteresis

wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "idle"    30
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running" 5

pass "Scenario 3: Pane1 transitions to Idle independently (Pane2 stays Running)"

# ── Scenario 4: Verify agtmux ls --group=session agent count ─────────────

SESS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls --group=session 2>/dev/null || echo "")
assert_contains "ls-session counts 2 agents" "2 agent" "$SESS_OUT"

kill "$INJECTOR_A_PID" 2>/dev/null || true
kill "$INJECTOR_B_PID" 2>/dev/null || true
INJECTOR_A_PID=""
INJECTOR_B_PID=""

pass "Scenario 4: agtmux ls --group=session shows correct agent count"

echo "=== test-multi-pane.sh PASS ==="
