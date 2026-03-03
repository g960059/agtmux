#!/usr/bin/env bash
# scenarios/codex-approval-flow.sh — Codex FSM approval (review mode) e2e test
#
# Tests the WaitingApproval FSM path that no existing scenario covers:
#   Phase 0: pane discovered -> presence=managed
#   Phase 1: task_started    -> activity_state=running
#   Phase 2: entered_review_mode -> activity_state=waiting_approval
#   Phase 3: exited_review_mode  -> activity_state=running (user approved)
#   Phase 4: entered_review_mode -> activity_state=waiting_approval (second tool)
#   Phase 5: task_started (override) -> activity_state=running
#            (task_started can fire from any state, including WaitingApproval)
#   Phase 6: turn_aborted -> activity_state=idle (Ctrl+C path)
#
# This validates:
#   - "waiting_approval" is exposed correctly via the JSON API (not just "running")
#   - The entered_review_mode -> exited_review_mode cycle works end-to-end
#   - task_started overrides WaitingApproval (wildcard FSM arm)
#   - turn_aborted produces idle (the Ctrl+C / interrupt path)
#
# Unit test coverage: fsm.rs::fsm_approval_flow tests the transitions.
# This test proves the full daemon stack propagates waiting_approval correctly
# through translate.rs -> Gateway -> DaemonProjection -> JSON API normalize.
#
# Technique: same as codex-semantic-states.sh (exec -a codex sleep + synthetic JSONL).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SESSION="e2e-codex-approval-$$"
SOCKET="/tmp/agtmux-e2e-codex-approval-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-codex-approval-workdir-$$"

echo "=== codex-approval-flow.sh (synthetic JSONL injection) ==="

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

# Clean up JSONL file and tmux session on exit
REAL_JSONL=""
cleanup_codex_approval() {
    [ -n "$REAL_JSONL" ] && rm -f "$REAL_JSONL"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_codex_approval EXIT

# -- Create synthetic JSONL session file --
CANONICAL_WORKDIR="$(cd "$WORKDIR" && pwd -P)"
REAL_CODEX_DATE_DIR="$HOME/.codex/sessions/$(date +%Y/%m/%d)"
mkdir -p "$REAL_CODEX_DATE_DIR"
REAL_JSONL="$REAL_CODEX_DATE_DIR/rollout-e2e-approval-test-$$.jsonl"
printf '{"type":"session_meta","payload":{"type":"session_meta","cwd":"%s","sessionId":"test-approval-%s"}}\n' \
    "$CANONICAL_WORKDIR" "$$" > "$REAL_JSONL"
log "JSONL file: $REAL_JSONL (cwd=$CANONICAL_WORKDIR)"

# -- Make the pane look like a Codex process --
tmux send-keys -t "$PANE_ID" "cd $(printf '%q' "$WORKDIR") && exec -a codex sleep 9999" Enter
sleep 1

PANE_PID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_pid}' 2>/dev/null | head -1)
log "pane_pid=$PANE_PID workdir=$WORKDIR"

daemon_start "$SOCKET" 500
sleep 2

# -- Phase 0: pane discovered -> presence=managed --
log "Phase 0: waiting for presence=managed"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10
pass "Phase 0: pane discovered and managed (deterministic)"

# -- Phase 1: task_started -> Running --
log "Phase 1: injecting task_started"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"task-appr-001"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 1: activity_state=running after task_started"

# -- Phase 2: entered_review_mode -> WaitingApproval --
log "Phase 2: injecting entered_review_mode -> expect waiting_approval"
printf '{"type":"event_msg","payload":{"type":"entered_review_mode","toolName":"bash","command":"rm -rf /tmp/test"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_approval" 30
pass "Phase 2: activity_state=waiting_approval after entered_review_mode"

# -- Phase 3: exited_review_mode -> Running (user approved) --
log "Phase 3: injecting exited_review_mode -> expect running"
printf '{"type":"event_msg","payload":{"type":"exited_review_mode","approved":true}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 3: activity_state=running after exited_review_mode"

# -- Phase 4: entered_review_mode again -> WaitingApproval --
log "Phase 4: second entered_review_mode -> expect waiting_approval"
printf '{"type":"event_msg","payload":{"type":"entered_review_mode","toolName":"write_file","path":"/etc/config"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_approval" 30
pass "Phase 4: activity_state=waiting_approval (second review)"

# -- Phase 5: task_started overrides WaitingApproval -> Running --
# This tests the wildcard FSM rule: (_, "event_msg", "task_started") => Running
# task_started can fire from ANY state, including WaitingApproval.
log "Phase 5: injecting task_started from WaitingApproval -> expect running"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"task-appr-002"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 5: activity_state=running (task_started overrides WaitingApproval)"

# -- Phase 6: turn_aborted -> WaitingInput (idle) --
# This tests the Ctrl+C / interrupt path: (Running | ToolExecuting, "event_msg", "turn_aborted") => WaitingInput
log "Phase 6: injecting turn_aborted -> expect idle"
printf '{"type":"event_msg","payload":{"type":"turn_aborted","reason":"user_cancelled"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30
pass "Phase 6: activity_state=idle after turn_aborted (Ctrl+C path)"

# -- Verify provider=codex and evidence_mode=deterministic throughout --
ACTUAL_PROVIDER=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Provider is codex" "codex" "$ACTUAL_PROVIDER"

ACTUAL_MODE=$(jq_get "$SOCKET" "$PANE_ID" "evidence_mode")
assert_eq "Evidence mode is deterministic" "deterministic" "$ACTUAL_MODE"

echo "=== codex-approval-flow.sh PASS ==="
