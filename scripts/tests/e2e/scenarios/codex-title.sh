#!/usr/bin/env bash
# scenarios/codex-title.sh — conversation_title for managed Codex exec panes
#
# Verifies: the daemon exposes a non-empty conversation_title while a Codex exec
# pane is managed, and exact shell demotion clears that title if Codex exits
# back to the shell.
#
# Note: `codex exec --json` pane capture drives semantic state, but the original
# prompt lives in the real Codex session transcript. The daemon must surface a
# stable title even when the pane-scoped exec spool is the active semantic path.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../providers/codex/adapter.sh"

register_cleanup

SESSION="e2e-codex-title-$$"
SOCKET="/tmp/agtmux-e2e-codex-title-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-codex-title-workdir-$$"

echo "=== codex-title.sh ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

cleanup_codex_title() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_codex_title EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: Launch Codex and wait for provider + managed presence ────────
#
# This scenario's primary oracle is conversation_title while the pane is still
# managed. Running/deterministic coverage is owned by the lifecycle / multi-pane
# scenarios.

TASK="Step 1: use bash to run 'sleep 10'. Step 2: use bash to count lines in /etc/hosts. Write the count to result.txt"
log "launching codex in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider"       "codex"         60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       60
wait_for_agtmux_state_any "$SOCKET" "$PANE_ID" "activity_state" "running waiting_input" 45

pass "Phase 1: Codex pane managed (provider=codex)"

# ── Phase 2: Verify conversation_title while managed ───────────────────────
#
# Allow up to 30s for the daemon to discover the Codex session transcript and
# expose a non-empty title on the managed pane row.

log "waiting for conversation_title from Codex session transcript..."
actual_title="null"
elapsed=0
while [ "$elapsed" -lt 30 ]; do
    actual_title=$(jq_get "$SOCKET" "$PANE_ID" "conversation_title")
    if [ "$actual_title" != "null" ] && [ -n "$actual_title" ]; then
        break
    fi
    sleep 2
    elapsed=$((elapsed + 2))
done

log "conversation_title='$actual_title'"

if [ "$actual_title" = "null" ] || [ -z "$actual_title" ]; then
    fail "conversation_title is null/empty for Codex pane — transcript-backed title extraction failed"
fi

pass "Phase 2: conversation_title='$actual_title' (non-empty while managed)"

# ── Phase 3: Completion may stay managed or demote back to shell ──────────

wait_until_provider_idle "$PANE_ID" 90 || log "WARN: provider-side idle check timed out"
completion_mode=$(wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 90)

if [ "$completion_mode" = "managed" ]; then
    final_title=$(jq_get "$SOCKET" "$PANE_ID" "conversation_title")
    if [ "$final_title" = "null" ] || [ -z "$final_title" ]; then
        fail "conversation_title cleared unexpectedly while Codex pane remained managed"
    fi
    pass "Phase 3: managed completion preserved conversation_title"
else
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "unmanaged" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "null" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "null" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "null" 5
    pass "Phase 3: shell demotion cleared conversation_title with managed state"
fi

echo "=== codex-title.sh PASS ==="
