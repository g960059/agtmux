#!/usr/bin/env bash
# scenarios/claude-title-after-restart.sh — conversation_title survives daemon restart
#
# Regression test for the "historical scan" bug:
#
#   BEFORE FIX: SessionFileWatcher::new() sought to EOF. If the daemon was
#   restarted while a session had JSONL history (including summary/custom-title
#   events), those events were silently skipped. conversation_title became null
#   for ALL managed panes on every daemon restart.
#
#   AFTER FIX: new() performs a one-time historical scan of 0..EOF to backfill
#   last_title, last_summary, and last_first_prompt before seeking to EOF for
#   real-time activity tracking.
#
# Test flow:
#   Phase 1: Start daemon, launch Claude, wait for managed + deterministic.
#   Phase 2: Locate JSONL file; inject a custom-title event (real-time path).
#   Phase 3: Verify conversation_title via real-time detection (control check).
#   Phase 4: Stop daemon. The JSONL file now contains the title event as history.
#   Phase 5: Start a FRESH daemon. Watcher seeks to EOF → historical scan must
#            backfill last_title. NO new events injected after restart.
#   Phase 6: Assert conversation_title is still populated from historical scan.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../providers/claude/adapter.sh"

register_cleanup

SESSION="e2e-title-restart-$$"
SOCKET="/tmp/agtmux-e2e-title-restart-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-title-restart-workdir-$$"

echo "=== claude-title-after-restart.sh ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

cleanup_restart() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_restart EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: Launch Claude, wait for managed + deterministic ───────────────

TASK="Step 1: use bash to run 'sleep 60'. Step 2: use bash to count lines in /etc/hosts. Write results to result.txt"
log "launching claude in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

wait_until_provider_running "$PANE_ID" 10 || log "WARN: provider-side running check timed out (non-fatal)"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider"      "claude"        60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"      "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 30

pass "Phase 1: Claude pane managed with deterministic evidence (watcher active)"

# ── Phase 2: Locate the JSONL file Claude is writing to ───────────────────

CANONICAL_WORKDIR=$(cd "$WORKDIR" && pwd -P 2>/dev/null || echo "$WORKDIR")
ENCODED_PATH=$(echo "$CANONICAL_WORKDIR" | tr '/.' '--')
CLAUDE_PROJECT_DIR="$HOME/.claude/projects/$ENCODED_PATH"
log "looking for JSONL in $CLAUDE_PROJECT_DIR (canonical: $CANONICAL_WORKDIR)"

JSONL_FILE=""
elapsed=0
while [ "$elapsed" -lt 30 ]; do
    JSONL_FILE=$(ls "$CLAUDE_PROJECT_DIR"/*.jsonl 2>/dev/null | head -1)
    [ -n "$JSONL_FILE" ] && break
    sleep 1
    elapsed=$((elapsed + 1))
done

[ -n "$JSONL_FILE" ] || fail "no JSONL file found in $CLAUDE_PROJECT_DIR after 30s"
log "found JSONL file: $JSONL_FILE"

pass "Phase 2: Claude JSONL file located"

# ── Phase 3: Inject title, verify real-time detection (control) ────────────
#
# This is the same as claude-title.sh: injects a custom-title event while the
# watcher is active and reading in real-time.

TITLE="Title That Must Survive Restart"
log "injecting custom-title: '$TITLE'"
printf '{"type":"custom-title","customTitle":"%s","sessionId":"e2e-restart-test"}\n' "$TITLE" >> "$JSONL_FILE"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "$TITLE" 15

pass "Phase 3: conversation_title='$TITLE' detected in real-time"

# ── Phase 4: Stop daemon ───────────────────────────────────────────────────
#
# The JSONL file now contains the custom-title event as historical data.
# Claude continues running (the 60s sleep is still active).

log "stopping daemon (JSONL history now contains the title event)"
daemon_stop
sleep 1

pass "Phase 4: daemon stopped (title is now pre-existing JSONL history)"

# ── Phase 5: Start a FRESH daemon ─────────────────────────────────────────
#
# The new daemon creates a fresh SessionFileWatcher for the same JSONL file.
# Without the historical-scan fix, it would seek to EOF and miss the injected
# custom-title event → conversation_title = null.
# With the fix, scan_historical() reads 0..EOF and backfills last_title.
#
# No new events are injected after the restart — the title must come entirely
# from the historical scan.

log "starting FRESH daemon (regression: watcher must read JSONL history)"
daemon_start "$SOCKET" 500
sleep 1

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"      "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider"      "claude"        30

pass "Phase 5: pane re-discovered as managed with fresh daemon"

# ── Phase 6: Assert conversation_title from historical scan ────────────────
#
# Regression gate: without the fix this would be "null" even though the
# custom-title event is present in the JSONL file.

actual_title=$(jq_get "$SOCKET" "$PANE_ID" "conversation_title")
assert_eq "Phase 6: conversation_title populated from JSONL history (historical scan)" \
    "$TITLE" "$actual_title"

echo "=== claude-title-after-restart.sh PASS ==="
