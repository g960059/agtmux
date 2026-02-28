#!/usr/bin/env bash
# contract/test-codex-state.sh — Codex state transitions via source.ingest injection
# Verifies: task.running → running (Deterministic), task.idle → idle transition.
# No real Codex CLI needed.
# activity_state values use snake_case (agtmux json schema v1).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-codex-$$"
SOCKET="/tmp/agtmux-e2e-codex-$$/agtmuxd.sock"
INJECTOR_PID=""

echo "=== test-codex-state.sh ==="

# ── Setup ─────────────────────────────────────────────────────────────────

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
if [ -z "$PANE_ID" ]; then
    fail "could not get pane_id from tmux session $SESSION"
fi
log "using pane_id=$PANE_ID session=$SESSION"

cleanup_tmux() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_tmux EXIT

daemon_start "$SOCKET" 500
sleep 1

THREAD_ID="e2e-thread-$$"
CWD="/tmp/e2e-codex-$$"
mkdir -p "$CWD"

# ── Scenario 1: task.running → running (Deterministic) ───────────────────
# Inject continuously to ensure freshness (resolver requires < 3s old events).

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "thread.active" "$THREAD_ID" "$PANE_ID" "$CWD")
log "thread.active injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 1: task.running → running (Deterministic)"

# ── Scenario 2: task.idle → idle ─────────────────────────────────────────

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "thread.idle" "$THREAD_ID" "$PANE_ID" "$CWD")
log "thread.idle injector PID=$INJECTOR_PID"
sleep 6  # hysteresis

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 2: task.idle → idle"

# ── Scenario 3: task.running again ───────────────────────────────────────

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "thread.active" "$THREAD_ID" "$PANE_ID" "$CWD")
log "thread.active (recovery) injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 20

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

pass "Scenario 3: task.running recovers to running after idle"

echo "=== test-codex-state.sh PASS ==="
