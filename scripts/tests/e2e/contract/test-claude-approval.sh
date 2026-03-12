#!/usr/bin/env bash
# contract/test-claude-approval.sh — Claude waiting_approval and waiting_input states
#
# Gap filled: NO e2e coverage for PermissionRequest → waiting_approval and
#   Stop/SubagentStop → waiting_input in the claude_hooks source.
#
# Precedence (projection.rs ActivityState):
#   Error(5) > WaitingApproval(4) > WaitingInput(3) > Running(2) > Idle(1)
#
# Phases:
#   1. tool_start → activity_state=running
#   2. PermissionRequest → activity_state=waiting_approval (wins because prec 4 > 2)
#   3. tool_start recovery → activity_state=running again
#   4. Stop → activity_state=waiting_input (wins because prec 3 > 2)
#
# Claude hooks translate.rs mappings tested here:
#   "PermissionRequest" → "activity.waiting_approval"
#   "Stop"              → "activity.waiting_input"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-claude-approval-$$"
SOCKET="/tmp/agtmux-e2e-claude-approval-$$/agtmuxd.sock"
SESSION_ID="e2e-approval-sess-$$"
INJECTOR_PID=""

echo "=== test-claude-approval.sh ==="

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane_id=$PANE_ID session=$SESSION"

cleanup_approval() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_approval EXIT

# Make the pane look like a node process (prevents shell-truth demotion).
make_pane_node_like "$PANE_ID"

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: tool_start → running ─────────────────────────────────────────

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "Phase 1: tool_start injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

pass "Phase 1: tool_start → running (Deterministic)"

# ── Phase 2: PermissionRequest → waiting_approval ─────────────────────────
# waiting_approval (precedence 4) wins over running (precedence 2).
# Switch injector to PermissionRequest; running events will be supplanted.

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "PermissionRequest" "$SESSION_ID" "$PANE_ID")
log "Phase 2: PermissionRequest injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_approval" 20

pass "Phase 2: PermissionRequest → waiting_approval"

# ── Phase 3: tool_start recovery → running ────────────────────────────────
# Stop PermissionRequest; switch back to tool_start (prec 2 < prec 4, so
# PermissionRequest events must age out first — but we stop the injector so
# they expire within <3s, then running wins).

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""
# Wait for PermissionRequest events to expire before starting tool_start injector.
# Explicit sleep avoids relying on implicit <3s event-expiry threshold.
sleep 4

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "Phase 3: recovery tool_start injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 20

pass "Phase 3: tool_start recovery → running after PermissionRequest stopped"

# ── Phase 4: Stop → waiting_input ─────────────────────────────────────────
# Stop hook fires when Claude finishes its response; pane waits for user input.
# waiting_input (precedence 3) wins over running (precedence 2), so switching
# the injector to Stop is sufficient.

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "Stop" "$SESSION_ID" "$PANE_ID")
log "Phase 4: Stop injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "waiting_input" 20

pass "Phase 4: Stop → waiting_input"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-claude-approval.sh PASS ==="
