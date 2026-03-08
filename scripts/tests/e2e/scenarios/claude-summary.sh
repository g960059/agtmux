#!/usr/bin/env bash
# scenarios/claude-summary.sh — conversation_title from Claude summary and sessions-index JSONL events
#
# Verifies (T-135c):
#   Phase 3: Injecting a `{"type":"summary",...}` JSONL event causes agtmux to
#            expose conversation_title from the AI-generated summary.
#   Phase 4: A subsequent `{"type":"custom-title",...}` line overrides the summary
#            (priority: custom-title > summary > sessions-index fallback).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../providers/claude/adapter.sh"

register_cleanup

SESSION="e2e-claude-summary-$$"
SOCKET="/tmp/agtmux-e2e-claude-summary-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-claude-summary-workdir-$$"

echo "=== claude-summary.sh ==="

# ── Setup ──────────────────────────────────────────────────────────────────

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

cleanup_claude_summary() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_claude_summary EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: Launch Claude, wait for managed + deterministic ───────────────
#
# Use evidence_mode=deterministic as the signal that the JSONL watcher is
# active. A long task (sleep 60) keeps Claude running long enough for us to
# inject events before it finishes.

TASK="Step 1: use bash to run 'sleep 60'. Step 2: use bash to count lines in /etc/hosts. Write results to result.txt"
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

# ── Phase 3: Inject summary event → conversation_title reflects AI summary ─
#
# The `type=summary` event carries an AI-generated session title in the
# `summary` field. agtmux should extract it and expose it as conversation_title.

SUMMARY_TITLE="E2E AI Generated Summary"
log "injecting summary event: '$SUMMARY_TITLE'"
printf '{"type":"summary","summary":"%s","leafUuid":"e2e-leaf-001"}\n' "$SUMMARY_TITLE" >> "$JSONL_FILE"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "$SUMMARY_TITLE" 15

pass "Phase 3: conversation_title='$SUMMARY_TITLE' (from summary event)"

# ── Phase 4: Inject custom-title → it must override the summary ────────────
#
# custom-title has higher priority than summary. After injecting a custom-title
# event, conversation_title must switch to the custom title.

CUSTOM_TITLE="Custom Title Wins Over Summary"
log "injecting custom-title event: '$CUSTOM_TITLE'"
printf '{"type":"custom-title","customTitle":"%s","sessionId":"e2e-summary-test"}\n' "$CUSTOM_TITLE" >> "$JSONL_FILE"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "conversation_title" "$CUSTOM_TITLE" 15

pass "Phase 4: conversation_title='$CUSTOM_TITLE' (custom-title overrides summary)"

echo "=== claude-summary.sh PASS ==="
