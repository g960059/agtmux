#!/usr/bin/env bash
# scenarios/same-cwd-multi-pane.sh — T-124 regression: two panes sharing the same CWD
#
# PROVIDER (env): codex (default; this scenario targets Codex thread-to-pane binding)
# Verifies: Both panes managed even when they share the same CWD.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER="${PROVIDER:-codex}"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

register_cleanup

SESSION="e2e-samecwd-${PROVIDER}-$$"
SOCKET="/tmp/agtmux-e2e-samecwd-${PROVIDER}-$$/agtmuxd.sock"
SHARED_CWD="/tmp/e2e-samecwd-$$"

echo "=== same-cwd-multi-pane.sh (PROVIDER=${PROVIDER}) [T-124 regression] ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$SHARED_CWD"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null
tmux split-window -h -t "$SESSION:main" 2>/dev/null

PANE_IDS=( $(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null) )
[ ${#PANE_IDS[@]} -ge 2 ] || fail "need at least 2 panes in session $SESSION"
PANE1="${PANE_IDS[0]}"
PANE2="${PANE_IDS[1]}"
log "pane1=$PANE1 pane2=$PANE2 shared_cwd=$SHARED_CWD provider=$PROVIDER"

cleanup_samecwd() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$SHARED_CWD"
}
trap cleanup_samecwd EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario: Two panes, SAME CWD, both Running ───────────────────────────

log "launching $PROVIDER in pane1=$PANE1 (shared_cwd=$SHARED_CWD)"
launch_provider "$PANE1" "$SHARED_CWD" "count lines in /etc/hosts and write to result1.txt"
sleep 2

log "launching $PROVIDER in pane2=$PANE2 (same shared_cwd=$SHARED_CWD)"
launch_provider "$PANE2" "$SHARED_CWD" "count lines in /etc/hosts and write to result2.txt"

# Provider-side
wait_until_provider_running "$PANE1" 60 || log "WARN: pane1 provider-side running check timed out"
wait_until_provider_running "$PANE2" 60 || log "WARN: pane2 provider-side running check timed out"

# agtmux-side: BOTH panes must be managed (T-124 regression)
wait_for_agtmux_state "$SOCKET" "$PANE1" "presence" "managed" 60
wait_for_agtmux_state "$SOCKET" "$PANE2" "presence" "managed" 60

pass "Scenario 1 (T-124 regression): Both panes managed even with shared CWD"

wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE1" "evidence_mode"  "deterministic" 30
wait_for_agtmux_state "$SOCKET" "$PANE2" "evidence_mode"  "deterministic" 30

pass "Scenario 2: Both panes Running with deterministic evidence"

echo "=== same-cwd-multi-pane.sh PASS (PROVIDER=${PROVIDER}) ==="
