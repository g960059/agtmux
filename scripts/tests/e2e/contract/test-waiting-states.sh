#!/usr/bin/env bash
# contract/test-waiting-states.sh — WaitingApproval/WaitingInput display as "Waiting"
# Verifies T-136 fix: agtmux json normalises to snake_case ("waiting_approval"/"waiting_input"),
# but agtmux ls / agtmux ls --group=session display normalizes both to "Waiting".
#
# Uses codex_appserver source with lifecycle.* event_types to drive the pane
# into WaitingApproval / WaitingInput states — no real CLI required.
#
# Injection chain:
#   inject_codex_event_loop  "lifecycle.waiting_approval"
#   → CodexRawEvent.event_type = "lifecycle.waiting_approval"   (String, no enum)
#   → translate() passes through unchanged
#   → parse_activity_state("lifecycle.waiting_approval") = ActivityState::WaitingApproval
#   → server serialises with Debug format → JSON field "WaitingApproval"
#   → agtmux json normalises → "waiting_approval"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-waiting-$$"
SOCKET="/tmp/agtmux-e2e-waiting-$$/agtmuxd.sock"
THREAD_ID="e2e-wait-thread-$$"
INJECTOR_PID=""

echo "=== test-waiting-states.sh ==="

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
if [ -z "$PANE_ID" ]; then
    fail "could not get pane_id from tmux session $SESSION"
fi
log "using pane_id=$PANE_ID thread_id=$THREAD_ID"

cleanup_waiting() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_waiting EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario 1: WaitingApproval → displayed as "Waiting" ──────────────────
# lifecycle.waiting_approval → parse_activity_state → ActivityState::WaitingApproval
# The server Debug-formats ActivityState → JSON value "WaitingApproval".
# agtmux json normalises "WaitingApproval" → "waiting_approval".
# agtmux ls / agtmux ls --group=session display → "Waiting".

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.waiting_approval" "$THREAD_ID" "$PANE_ID")
log "WaitingApproval injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"          30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_approval" 30

pass "Scenario 1: pane reached waiting_approval"

# agtmux ls: normalised → "Waiting", raw variant hidden
LS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls 2>/dev/null || echo "")
assert_contains     "ls shows Waiting"          "Waiting"         "$LS_OUT"
assert_not_contains "ls hides WaitingApproval"  "WaitingApproval" "$LS_OUT"
assert_not_contains "ls hides WaitingInput"     "WaitingInput"    "$LS_OUT"

pass "Scenario 1a: agtmux ls normalisation (WaitingApproval → Waiting)"

# agtmux ls --group=session: counted under "Waiting"
SESS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls --group=session 2>/dev/null || echo "")
assert_contains     "ls-session shows Waiting"         "Waiting"         "$SESS_OUT"
assert_not_contains "ls-session hides WaitingApproval" "WaitingApproval" "$SESS_OUT"

pass "Scenario 1b: agtmux ls --group=session normalisation (WaitingApproval → Waiting)"

# agtmux json: snake_case value preserved for tooling / scripting
RAW_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "JSON schema v1: activity_state" "waiting_approval" "$RAW_STATE"

pass "Scenario 1c: JSON schema v1 value (waiting_approval)"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""
sleep 1   # allow in-flight events to settle before switching state

# ── Scenario 2: WaitingInput → displayed as "Waiting" ─────────────────────
# Reuse same THREAD_ID so the projection updates the existing session's state.

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.waiting_input" "$THREAD_ID" "$PANE_ID")
log "WaitingInput injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_input" 30

pass "Scenario 2: pane reached waiting_input"

LS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls 2>/dev/null || echo "")
assert_contains     "ls shows Waiting (WaitingInput)" "Waiting"     "$LS_OUT"
assert_not_contains "ls hides WaitingInput"           "WaitingInput" "$LS_OUT"

pass "Scenario 2a: agtmux ls normalisation (WaitingInput → Waiting)"

SESS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls --group=session 2>/dev/null || echo "")
assert_contains     "ls-session shows Waiting (WaitingInput)" "Waiting"     "$SESS_OUT"
assert_not_contains "ls-session hides WaitingInput"           "WaitingInput" "$SESS_OUT"

pass "Scenario 2b: agtmux ls --group=session normalisation (WaitingInput → Waiting)"

RAW_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "JSON schema v1: activity_state" "waiting_input" "$RAW_STATE"

pass "Scenario 2c: JSON schema v1 value (waiting_input)"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-waiting-states.sh PASS ==="
