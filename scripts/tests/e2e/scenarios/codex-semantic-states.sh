#!/usr/bin/env bash
# scenarios/codex-semantic-states.sh — Codex FSM semantic state detection e2e test
#
# Tests that agtmux correctly detects all Codex FSM states using the new
# agtmux-source-codex-jsonl implementation (semantic JSONL parsing).
#
# States tested:
#   Phase 0: pane discovered → presence=managed (idle_heartbeat from Init state)
#   Phase 1: task_started    → activity_state = running
#   Phase 2: task_complete   → activity_state = idle (WaitingInput)
#   Phase 3: new task_started → running again
#
# Mechanism:
#   1. Creates a tmux pane running "exec -a codex sleep 9999" so
#      inspect_pane_processes("codex") → process_hint=codex → codex-jsonl source
#      includes the pane in its discovery loop.
#   2. Places a synthetic JSONL file in ~/.codex/sessions/ with session_meta.payload.cwd
#      matching the pane's CWD.
#   3. Appends JSONL events to the file; the daemon's FileWatcher picks them up and
#      drives the FSM, which emits SourceEventV2 events to the Gateway.
#
# No real Codex CLI needed.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SESSION="e2e-codex-semantic-$$"
SOCKET="/tmp/agtmux-e2e-codex-sem-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-codex-sem-workdir-$$"

echo "=== codex-semantic-states.sh (synthetic JSONL injection) ==="

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

# Clean up JSONL file and tmux session on exit
REAL_JSONL=""
cleanup_codex_sem() {
    [ -n "$REAL_JSONL" ] && rm -f "$REAL_JSONL"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_codex_sem EXIT

# ── Create synthetic JSONL session file ─────────────────────────────────────
CANONICAL_WORKDIR="$(cd "$WORKDIR" && pwd -P)"
REAL_CODEX_DATE_DIR="$HOME/.codex/sessions/$(date +%Y/%m/%d)"
mkdir -p "$REAL_CODEX_DATE_DIR"
REAL_JSONL="$REAL_CODEX_DATE_DIR/rollout-e2e-semantic-test-$$.jsonl"
printf '{"type":"session_meta","payload":{"type":"session_meta","cwd":"%s","sessionId":"test-sem-%s"}}\n' \
    "$CANONICAL_WORKDIR" "$$" > "$REAL_JSONL"
log "JSONL file: $REAL_JSONL (cwd=$CANONICAL_WORKDIR)"

# ── Make the pane look like a Codex process ──────────────────────────────────
# exec -a codex sleep 9999:
#   - Replaces the shell with a sleep process named "codex"
#   - #{pane_current_command} → "codex"
#   - inspect_pane_processes("codex") → process_hint = Some("codex")
#   - poll_loop.rs filter includes this pane in codex-jsonl discovery
#   - CWD of the process = $WORKDIR (we cd there before exec)
tmux send-keys -t "$PANE_ID" "cd $(printf '%q' "$WORKDIR") && exec -a codex sleep 9999" Enter
sleep 1  # wait for exec to replace the shell; #{pane_pid} now points to sleep

PANE_PID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_pid}' 2>/dev/null | head -1)
log "pane_pid=$PANE_PID workdir=$WORKDIR"

daemon_start "$SOCKET" 500
sleep 2

# ── Phase 0: pane discovered → presence=managed ──────────────────────────────
# The daemon discovers the pane (process_hint=codex), reads session_meta,
# emits an idle_heartbeat (Init state), making the pane managed.
log "Phase 0: waiting for presence=managed (idle_heartbeat from Init)"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10
pass "Phase 0: pane discovered and managed (deterministic)"

# ── Phase 1: task_started → Running ──────────────────────────────────────────
log "Phase 1: injecting task_started → expect running"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"task-001"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 1: activity_state=running after task_started"

# ── Phase 2: task_complete → WaitingInput (idle) ─────────────────────────────
log "Phase 2: injecting task_complete → expect idle"
printf '{"type":"event_msg","payload":{"type":"task_complete"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30
pass "Phase 2: activity_state=idle after task_complete"

# ── Phase 3: user_message + task_started → Running again ─────────────────────
log "Phase 3: second task_started → expect running again"
printf '{"type":"event_msg","payload":{"type":"user_message","content":"run again"}}\n' >> "$REAL_JSONL"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"task-002"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30
pass "Phase 3: activity_state=running after second task_started"

echo "=== codex-semantic-states.sh PASS ==="
