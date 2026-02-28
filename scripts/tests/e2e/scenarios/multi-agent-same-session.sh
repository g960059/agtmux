#!/usr/bin/env bash
# scenarios/multi-agent-same-session.sh — Two agents in the same tmux session, different CWDs
#
# PROVIDER (env): claude | codex  (default: claude)
# Verifies: Both panes managed independently in the same session.

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
launch_provider "$PANE1" "$WORKDIR1"
sleep 2  # stagger slightly

log "launching $PROVIDER in pane2=$PANE2 (workdir2=$WORKDIR2)"
launch_provider "$PANE2" "$WORKDIR2"

# Provider-side signals (best-effort)
wait_until_provider_running "$PANE1" 60 || log "WARN: pane1 provider-side running check timed out"
wait_until_provider_running "$PANE2" 60 || log "WARN: pane2 provider-side running check timed out"

# agtmux-side: both panes managed, Running, and deterministic
wait_for_agtmux_state "$SOCKET" "$PANE1" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE2" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE1" "evidence_mode"  "deterministic" 30
wait_for_agtmux_state "$SOCKET" "$PANE2" "evidence_mode"  "deterministic" 30

pass "Scenario 1: Both $PROVIDER agents managed independently in same session (deterministic)"

# Wait for both to finish
wait_until_provider_idle "$PANE1" 180 || log "WARN: pane1 idle check timed out"
wait_until_provider_idle "$PANE2" 180 || log "WARN: pane2 idle check timed out"

wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "Idle" 60
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "Idle" 60

pass "Scenario 2: Both $PROVIDER agents Idle after completion"

echo "=== multi-agent-same-session.sh PASS (PROVIDER=${PROVIDER}) ==="
