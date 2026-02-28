#!/usr/bin/env bash
# scenarios/single-agent-lifecycle.sh — Running → Idle lifecycle for a single agent
#
# PROVIDER (env): claude | codex  (default: claude)
# Verifies: managed presence, Running detection, evidence_mode, Idle after completion.

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
launch_provider "$PANE_ID" "$WORKDIR"

# Provider-side: wait until provider has started outputting (adapter-specific)
wait_until_provider_running "$PANE_ID" 60 || log "WARN: provider-side running check timed out (non-fatal)"

# agtmux-side: presence → Running → evidence_mode
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Running"       45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 30

pass "Scenario 1: $PROVIDER detected as Running (Deterministic)"

# Provider-side: wait until provider has finished (adapter-specific)
wait_until_provider_idle "$PANE_ID" 180 || log "WARN: provider-side idle check timed out (non-fatal)"

# agtmux-side: Idle after completion
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "Idle" 60

pass "Scenario 2: $PROVIDER detected as Idle after completion"

echo "=== single-agent-lifecycle.sh PASS (PROVIDER=${PROVIDER}) ==="
