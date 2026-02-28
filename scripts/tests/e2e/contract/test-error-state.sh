#!/usr/bin/env bash
# contract/test-error-state.sh — lifecycle.error → ActivityState::Error display
#
# Verifies:
#   1. lifecycle.error event produces activity_state="error" in agtmux json (schema v1)
#   2. agtmux ls shows "Error" as-is (unlike WaitingApproval which normalises to "Waiting")
#   3. JSON schema v1 value "error" is preserved in agtmux json
#   4. Error state transitions: Running → Error and Error → Running recovery

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-error-$$"
SOCKET="/tmp/agtmux-e2e-error-$$/agtmuxd.sock"
THREAD_ID="e2e-error-thread-$$"
INJECTOR_PID=""

echo "=== test-error-state.sh ==="

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane_id=$PANE_ID thread_id=$THREAD_ID"

cleanup_error() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_error EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario 1: lifecycle.error → ActivityState::Error ────────────────────
# parse_activity_state("lifecycle.error") → ActivityState::Error
# Server Debug-formats ActivityState → JSON "Error"
# agtmux json normalises "Error" → "error"
# agtmux ls display_state: "Error" passes through unchanged (no normalisation)

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.error" "$THREAD_ID" "$PANE_ID")
log "Error injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"  30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "error"    30

pass "Scenario 1: pane reached error state"

# agtmux ls: "Error" shown as-is (no normalisation like WaitingApproval)
LS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls 2>/dev/null || echo "")
assert_contains     "ls shows Error"             "Error"          "$LS_OUT"
assert_not_contains "ls hides WaitingApproval"   "WaitingApproval" "$LS_OUT"

pass "Scenario 1a: agtmux ls displays Error state"

# JSON schema v1 value: "error" (snake_case)
RAW_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "JSON schema v1: activity_state" "error" "$RAW_STATE"

pass "Scenario 1b: JSON schema v1 value (error)"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""
sleep 1

# ── Scenario 2: Running → Error transition ─────────────────────────────────

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.running" "$THREAD_ID" "$PANE_ID")
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 20
kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""
sleep 1

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.error" "$THREAD_ID" "$PANE_ID")
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "error" 20

pass "Scenario 2: Running → Error transition"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""
sleep 1

# ── Scenario 3: Error → Running recovery ──────────────────────────────────
# ActivityState precedence: Error(5) > WaitingApproval(4) > ... > Running(2)
# But latest-event wins within the same session — new Running events override Error.

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.running" "$THREAD_ID" "$PANE_ID")
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 20

pass "Scenario 3: Error → Running recovery"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-error-state.sh PASS ==="
