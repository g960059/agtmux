#!/usr/bin/env bash
# scenarios/claude-title.sh — conversation_title from Claude custom-title JSONL events
#
# Verifies: When custom-title events are appended to Claude's active JSONL file
# (mirroring what /rename does), agtmux extracts them and exposes conversation_title
# in JSON output. Also verifies that the last title wins on back-to-back updates.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../providers/claude/adapter.sh"

register_cleanup

SESSION="e2e-claude-title-$$"
SOCKET="/tmp/agtmux-e2e-claude-title-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-claude-title-workdir-$$"

echo "=== claude-title.sh ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

cleanup_claude_title() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_claude_title EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: Launch Claude, wait for managed + deterministic ───────────────
#
# We use evidence_mode=deterministic (not activity_state=running) as the signal
# that the JSONL watcher is active. This is robust whether Claude is running or
# already idle — the watcher emits deterministic events in both cases.
#
# Use a long sleep (60s) to ensure the watcher is still alive when we inject titles.

TASK="Step 1: use bash to run 'sleep 60'. Step 2: use bash to count lines in /etc/hosts. Step 3: use bash to count lines in /etc/shells. Write all results to result.txt"
log "launching claude in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

wait_until_provider_running "$PANE_ID" 10 || log "WARN: provider-side running check timed out (non-fatal)"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider"      "claude"        60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"      "managed"       60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 30

pass "Phase 1: Claude pane managed with deterministic evidence (watcher active)"

# ── Phase 2: Locate the JSONL file Claude is writing to ───────────────────
#
# Claude stores transcripts under:
#   ~/.claude/projects/<encode_path(canonical_cwd)>/<session-id>.jsonl
#
# encode_path() replaces '/' and '.' with '-'.
# On macOS, /tmp is a symlink to /private/tmp; canonicalize first.

CANONICAL_WORKDIR=$(cd "$WORKDIR" && pwd -P 2>/dev/null || echo "$WORKDIR")
# Replace '/' and '.' with '-' (same as Rust encode_path())
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

# ── Phase 3: Inject title1 then title2 back-to-back ───────────────────────
#
# Write both custom-title lines immediately one after the other so the watcher
# reads them in the same poll cycle. The last title seen wins. This avoids a
# race condition where Claude finishes between two separate injection steps.

TITLE1="E2E Test Session Title"
TITLE2="Updated E2E Title"
log "injecting custom-title lines: '$TITLE1' then '$TITLE2'"
printf '{"type":"custom-title","customTitle":"%s","sessionId":"e2e-test"}\n' "$TITLE1" >> "$JSONL_FILE"
printf '{"type":"custom-title","customTitle":"%s","sessionId":"e2e-test"}\n' "$TITLE2" >> "$JSONL_FILE"

# Wait for the last title (title2) — proves both lines were processed in order.
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "$TITLE2" 15

pass "Phase 3: conversation_title='$TITLE2' (back-to-back injection, last wins)"

# ── Phase 4: Wait for idle + verify title persists ────────────────────────

wait_until_provider_idle "$PANE_ID" 90 || log "WARN: provider-side idle check timed out (non-fatal)"
completion_mode=$(wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 90)

if [ "$completion_mode" = "managed" ]; then
    actual_title=$(jq_get "$SOCKET" "$PANE_ID" "conversation_title")
    assert_eq "Phase 4: conversation_title persists after idle" "$TITLE2" "$actual_title"
else
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "unmanaged" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "null" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "null" 5
    wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "null" 5
    pass "Phase 4: shell demotion cleared conversation_title with managed state"
fi

echo "=== claude-title.sh PASS ==="
