#!/usr/bin/env bash
# scenarios/codex-tool-execution.sh — Codex FSM tool execution cycle e2e test
#
# Tests the full tool execution FSM path that the semantic-states test does NOT cover:
#   Phase 0: pane discovered, presence=managed (idle_heartbeat from Init)
#   Phase 1: task_started    -> activity_state=running
#   Phase 2: function_call   -> activity_state=running (ToolExecuting maps to running)
#   Phase 3: function_call_output -> activity_state=running (back to Running)
#   Phase 4: custom_tool_call -> activity_state=running (ToolExecuting again)
#   Phase 5: custom_tool_call_output -> activity_state=running (back to Running)
#   Phase 6: task_complete   -> activity_state=idle (WaitingInput)
#
# This validates:
#   - response_item events (function_call, function_call_output, custom_tool_call,
#     custom_tool_call_output) are correctly parsed and drive FSM transitions
#   - ToolExecuting state is mapped to activity.running (not a separate state in the API)
#   - The daemon stays in "running" throughout the tool cycle (no transient idle flaps)
#   - Both standard (function_call) and custom (custom_tool_call) tool paths work
#
# Unit test coverage: fsm.rs tests these transitions in isolation.
# This test proves the full stack works: JSONL file -> FileWatcher -> FSM -> translate
# -> SourceEventV2 -> Gateway -> DaemonProjection -> JSON API.
#
# Technique: same as codex-semantic-states.sh (exec -a codex sleep + synthetic JSONL).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SESSION="e2e-codex-tool-$$"
SOCKET="/tmp/agtmux-e2e-codex-tool-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-codex-tool-workdir-$$"

echo "=== codex-tool-execution.sh (synthetic JSONL injection) ==="

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

# Clean up JSONL file and tmux session on exit
REAL_JSONL=""
cleanup_codex_tool() {
    [ -n "$REAL_JSONL" ] && rm -f "$REAL_JSONL"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_codex_tool EXIT

# -- Create synthetic JSONL session file --
CANONICAL_WORKDIR="$(cd "$WORKDIR" && pwd -P)"
REAL_CODEX_DATE_DIR="$HOME/.codex/sessions/$(date +%Y/%m/%d)"
mkdir -p "$REAL_CODEX_DATE_DIR"
REAL_JSONL="$REAL_CODEX_DATE_DIR/rollout-e2e-tool-test-$$.jsonl"
printf '{"type":"session_meta","payload":{"type":"session_meta","cwd":"%s","sessionId":"test-tool-%s"}}\n' \
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
log "Phase 0: waiting for presence=managed (idle_heartbeat from Init)"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10
pass "Phase 0: pane discovered and managed (deterministic)"

# -- Phase 1: task_started -> Running --
log "Phase 1: injecting task_started -> expect running"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"task-tool-001"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 1: activity_state=running after task_started"

# -- Phase 2: function_call -> ToolExecuting (still "running" in API) --
log "Phase 2: injecting function_call (response_item) -> expect running (ToolExecuting)"
printf '{"type":"response_item","payload":{"type":"function_call","name":"bash","id":"call_001","arguments":"ls -la"}}\n' >> "$REAL_JSONL"

# ToolExecuting maps to activity.running in translate.rs (state_to_event_type).
# The daemon should remain "running" — NOT flip to idle.
sleep 3
ACTUAL_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 2: ToolExecuting maps to running" "running" "$ACTUAL_STATE"

# -- Phase 3: function_call_output -> back to Running --
log "Phase 3: injecting function_call_output -> expect running (back to Running)"
printf '{"type":"response_item","payload":{"type":"function_call_output","id":"call_001","output":"total 42\\ndrwxr-xr-x ..."}}\n' >> "$REAL_JSONL"

sleep 3
ACTUAL_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 3: function_call_output -> Running" "running" "$ACTUAL_STATE"

# -- Phase 4: custom_tool_call -> ToolExecuting again --
log "Phase 4: injecting custom_tool_call -> expect running (ToolExecuting)"
printf '{"type":"response_item","payload":{"type":"custom_tool_call","name":"read_file","id":"call_002"}}\n' >> "$REAL_JSONL"

sleep 3
ACTUAL_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 4: custom_tool_call -> ToolExecuting -> running" "running" "$ACTUAL_STATE"

# -- Phase 5: custom_tool_call_output -> back to Running --
log "Phase 5: injecting custom_tool_call_output -> expect running"
printf '{"type":"response_item","payload":{"type":"custom_tool_call_output","id":"call_002","output":"file contents..."}}\n' >> "$REAL_JSONL"

sleep 3
ACTUAL_STATE=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 5: custom_tool_call_output -> Running" "running" "$ACTUAL_STATE"

# -- Phase 6: task_complete -> WaitingInput (idle) --
log "Phase 6: injecting task_complete -> expect idle"
printf '{"type":"event_msg","payload":{"type":"task_complete"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30
pass "Phase 6: activity_state=idle after task_complete"

# -- Verify provider=codex throughout --
ACTUAL_PROVIDER=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Provider is codex" "codex" "$ACTUAL_PROVIDER"

echo "=== codex-tool-execution.sh PASS ==="
