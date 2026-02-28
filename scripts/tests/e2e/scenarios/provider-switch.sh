#!/usr/bin/env bash
# scenarios/provider-switch.sh — PROVIDER_A stops, PROVIDER_B starts in same pane
#
# Default: Claude stops, Codex starts (PROVIDER_A=claude PROVIDER_B=codex)
# Tests agtmux's cross-provider arbitration (select_winning_provider in projection).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER_A="${PROVIDER_A:-claude}"
PROVIDER_B="${PROVIDER_B:-codex}"

register_cleanup

SESSION="e2e-switch-${PROVIDER_A}-${PROVIDER_B}-$$"
SOCKET="/tmp/agtmux-e2e-switch-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-switch-workdir-$$"

echo "=== provider-switch.sh (${PROVIDER_A} → ${PROVIDER_B}) ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION providers=${PROVIDER_A}→${PROVIDER_B}"

cleanup_switch() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_switch EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: PROVIDER_A runs and finishes ─────────────────────────────────

source "$SCRIPT_DIR/../providers/${PROVIDER_A}/adapter.sh"
log "Phase 1: launching $PROVIDER_A"
launch_provider "$PANE_ID" "$WORKDIR" "count lines in /etc/hosts and write to phase1.txt"

wait_until_provider_running "$PANE_ID" 60 || log "WARN: provider_a running check timed out"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 30

pass "Phase 1: $PROVIDER_A Running (deterministic)"

wait_until_provider_idle "$PANE_ID" 180 || log "WARN: provider_a idle check timed out"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Idle" 60

pass "Phase 2: $PROVIDER_A Idle (completed)"

# Unset provider A functions before loading provider B
unset -f launch_provider wait_until_provider_running wait_until_provider_idle 2>/dev/null || true

# ── Phase 2: PROVIDER_B runs in same pane ─────────────────────────────────

source "$SCRIPT_DIR/../providers/${PROVIDER_B}/adapter.sh"
log "Phase 2: launching $PROVIDER_B in same pane"
launch_provider "$PANE_ID" "$WORKDIR" "count lines in /etc/hosts and write to phase2.txt"

wait_until_provider_running "$PANE_ID" 60 || log "WARN: provider_b running check timed out"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Running"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 30

pass "Phase 3: $PROVIDER_B Running (provider switched in same pane, deterministic)"

wait_until_provider_idle "$PANE_ID" 180 || log "WARN: provider_b idle check timed out"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Idle" 60

pass "Phase 4: $PROVIDER_B Idle"

echo "=== provider-switch.sh PASS (${PROVIDER_A} → ${PROVIDER_B}) ==="
