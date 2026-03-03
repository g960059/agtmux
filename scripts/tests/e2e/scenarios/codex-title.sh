#!/usr/bin/env bash
# scenarios/codex-title.sh — conversation_title from Codex App Server thread/list
#
# Verifies: After a Codex session completes, the thread name/preview returned by
# the App Server thread/list endpoint is exposed as conversation_title in JSON.
#
# Note: Codex does not support in-session rename like Claude's /rename.
# The title (name/preview) is set by the App Server from the thread metadata.

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

# ── Phase 1: Launch Codex and wait for running + deterministic ────────────
#
# sleep 10 ensures the session stays alive long enough to observe the running state.

TASK="Step 1: use bash to run 'sleep 10'. Step 2: use bash to count lines in /etc/hosts. Write the count to result.txt"
log "launching codex in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

wait_until_provider_running "$PANE_ID" 10 || log "WARN: provider-side running check timed out"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 30

pass "Phase 1: Codex running and managed (deterministic)"

# ── Phase 2: Wait for Codex to complete ──────────────────────────────────

wait_until_provider_idle "$PANE_ID" 90 || log "WARN: provider-side idle check timed out"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_input" 90

pass "Phase 2: Codex completed task"

# ── Phase 3: Verify conversation_title from App Server ────────────────────
#
# After completion, App Server thread/list returns the thread with name/preview fields.
# Allow up to 30s for the App Server to report the completed thread and for
# the daemon to extract and expose it.

log "waiting for conversation_title from App Server thread/list..."
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
    fail "conversation_title is null/empty for Codex pane — App Server thread/list may not return name/preview fields"
fi

pass "Phase 3: conversation_title='$actual_title' (non-empty, from App Server)"

echo "=== codex-title.sh PASS ==="
