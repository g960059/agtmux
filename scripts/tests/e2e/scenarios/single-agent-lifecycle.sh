#!/usr/bin/env bash
# scenarios/single-agent-lifecycle.sh — Running → completed lifecycle for a single agent
#
# PROVIDER (env): claude | codex  (default: claude)
# Verifies: provider, managed presence, Running detection, evidence_mode, completion state.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER="${PROVIDER:-claude}"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

register_cleanup

SESSION="e2e-online-${PROVIDER}-$$"
SOCKET="/tmp/agtmux-e2e-${PROVIDER}-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-workdir-$$"

echo "=== single-agent-lifecycle.sh (PROVIDER=${PROVIDER}) ==="

TASK="Run exactly one bash command and do not run any additional commands. Wait 30 seconds by using sleep 30. bash -lc 'sleep 30; printf \"wait_result=idle\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=idle"

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION provider=$PROVIDER"

cleanup_online() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_online EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Scenario: launch → Running → Idle ─────────────────────────────────────

log "launching $PROVIDER in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

# agtmux-side oracle: capture the running edge before short tasks complete.
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider"       "$PROVIDER"      60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 30

pass "Scenario 1: $PROVIDER detected as running (deterministic)"

# Provider-side: wait until provider has finished (adapter-specific)
wait_until_provider_idle "$PANE_ID" 30 || log "WARN: provider-side idle check timed out (non-fatal)"

# agtmux-side: provider either completed in-pane or the row demoted back to shell truth.
wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 60 >/dev/null

pass "Scenario 2: $PROVIDER detected as completed after completion"

echo "=== single-agent-lifecycle.sh PASS (PROVIDER=${PROVIDER}) ==="
