#!/usr/bin/env bash
# scenarios/multi-agent-same-session.sh — Two agents in the same tmux session, different CWDs
#
# PROVIDER (env): claude | codex  (default: claude)
# Verifies: Both panes keep correct provider/state independently in the same session.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER="${PROVIDER:-claude}"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

register_cleanup

SESSION="e2e-multi-${PROVIDER}-$$"
SOCKET="/tmp/agtmux-e2e-multi-${PROVIDER}-$$/agtmuxd.sock"
WORKDIR1="/tmp/e2e-workdir1-$$"
WORKDIR2="/tmp/e2e-workdir2-$$"

echo "=== multi-agent-same-session.sh (PROVIDER=${PROVIDER}) ==="

TASK1="Run exactly one bash command and do not run any additional commands. Wait 30 seconds by using sleep 30. bash -lc 'sleep 30; printf \"wait_result=pane1\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=pane1"
TASK2="Run exactly one bash command and do not run any additional commands. Wait 30 seconds by using sleep 30. bash -lc 'sleep 30; printf \"wait_result=pane2\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=pane2"

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR1" "$WORKDIR2"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null
tmux split-window -h -t "$SESSION:main" 2>/dev/null

PANE_IDS=( $(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null) )
[ ${#PANE_IDS[@]} -ge 2 ] || fail "need at least 2 panes in session $SESSION"
PANE1="${PANE_IDS[0]}"
PANE2="${PANE_IDS[1]}"
log "pane1=$PANE1 pane2=$PANE2 session=$SESSION provider=$PROVIDER"

cleanup_multi() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR1" "$WORKDIR2"
}
trap cleanup_multi EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario: launch two agents → both Running ────────────────────────────

log "launching $PROVIDER in pane1=$PANE1 (workdir1=$WORKDIR1)"
launch_provider "$PANE1" "$WORKDIR1" "$TASK1"

# Observe pane1 first so short-lived runs do not complete before pane2 starts.
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "running"       45
wait_for_agtmux_state "$SOCKET" "$PANE1" "provider"       "$PROVIDER"      60
wait_for_agtmux_state "$SOCKET" "$PANE1" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE1" "evidence_mode"  "deterministic" 30

log "launching $PROVIDER in pane2=$PANE2 (workdir2=$WORKDIR2)"
launch_provider "$PANE2" "$WORKDIR2" "$TASK2"

# agtmux-side oracle: ensure pane2 also reaches deterministic running.
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running"       45
wait_for_agtmux_state "$SOCKET" "$PANE2" "provider"       "$PROVIDER"      60
wait_for_agtmux_state "$SOCKET" "$PANE2" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE2" "evidence_mode"  "deterministic" 30

pass "Scenario 1: Both $PROVIDER agents managed independently in same session (deterministic)"

# Wait for both to finish
wait_until_provider_idle "$PANE1" 30 || log "WARN: pane1 idle check timed out"
wait_until_provider_idle "$PANE2" 30 || log "WARN: pane2 idle check timed out"

wait_for_completion_or_shell_demotion "$SOCKET" "$PANE1" 60 >/dev/null
wait_for_completion_or_shell_demotion "$SOCKET" "$PANE2" 60 >/dev/null

pass "Scenario 2: Both $PROVIDER agents completed after completion"

echo "=== multi-agent-same-session.sh PASS (PROVIDER=${PROVIDER}) ==="
