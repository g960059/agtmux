#!/usr/bin/env bash
# scenarios/source-competition.sh — Claude hooks vs Codex JSONL on the SAME pane
#
# Gap filled: ZERO e2e coverage for the cross-provider arbitration logic
#   (select_winning_provider in projection.rs, T-123). This tests what happens
#   when TWO deterministic sources claim the same tmux pane simultaneously:
#     - Claude hooks (rank 0 for Claude) via UDS source.ingest injection
#     - Codex JSONL (rank 0 for Codex) via synthetic JSONL file scanning
#
# Why it matters: In real usage, a user might:
#   1. Run Codex in a pane (JSONL source discovers it as Codex)
#   2. Stop Codex and start Claude in the SAME pane (hooks source claims it as Claude)
#   The daemon must correctly switch the pane's provider from Codex → Claude.
#   If arbitration fails, the pane stays showing "Codex" even though Claude is running.
#
# Arbitration rules (projection.rs select_winning_provider):
#   - When only 1 Det provider exists → that provider wins (no conflict)
#   - When 2+ Det providers exist → provider with most recent non-heartbeat activity wins
#
# Test flow:
#   Phase 1: Start with Codex JSONL active → provider=codex, deterministic
#   Phase 2: Start Claude hooks on the SAME pane → provider switches to claude
#   Phase 3: Stop Claude hooks, Codex continues → provider switches back to codex
#   Phase 4: Verify final state is consistent
#
# Technique:
#   Codex: "exec -a codex sleep 9999" + synthetic JSONL in ~/.codex/sessions/
#   Claude: inject_claude_event_loop via UDS source.ingest
#
# NOTE: This test requires both:
#   - tmux pane running a fake codex process (for JSONL discovery)
#   - UDS injection of claude_hooks events (for hook source)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-src-compete-$$"
SOCKET="/tmp/agtmux-e2e-src-compete-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-src-compete-workdir-$$"
CLAUDE_INJECTOR_PID=""
REAL_JSONL=""

echo "=== source-competition.sh (Claude hooks vs Codex JSONL) ==="

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"

cleanup_compete() {
    [ -n "$CLAUDE_INJECTOR_PID" ] && kill "$CLAUDE_INJECTOR_PID" 2>/dev/null || true
    [ -n "$REAL_JSONL" ] && rm -f "$REAL_JSONL"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_compete EXIT

# ── Codex JSONL setup ──────────────────────────────────────────────────
# Create synthetic Codex JSONL so the codex-jsonl source discovers this pane.

CANONICAL_WORKDIR="$(cd "$WORKDIR" && pwd -P)"
REAL_CODEX_DATE_DIR="$HOME/.codex/sessions/$(date +%Y/%m/%d)"
mkdir -p "$REAL_CODEX_DATE_DIR"

REAL_JSONL="$REAL_CODEX_DATE_DIR/rollout-e2e-compete-$$.jsonl"
printf '{"type":"session_meta","payload":{"type":"session_meta","cwd":"%s","sessionId":"compete-codex-%s"}}\n' \
    "$CANONICAL_WORKDIR" "$$" > "$REAL_JSONL"
log "Codex JSONL: $REAL_JSONL (cwd=$CANONICAL_WORKDIR)"

# Make the pane look like a Codex process for discovery.
tmux send-keys -t "$PANE_ID" "cd $(printf '%q' "$WORKDIR") && exec -a codex sleep 9999" Enter
sleep 1

PANE_PID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_pid}' 2>/dev/null | head -1)
log "pane_pid=$PANE_PID"

daemon_start "$SOCKET" 500
sleep 2

# ── Phase 1: Codex JSONL active → provider=codex ──────────────────────
# Wait for the daemon to discover the pane first (from session_meta),
# then inject task_started AFTER discovery. The watcher starts reading
# from the end of file on creation, so events appended after watcher
# creation are read. Events before watcher creation are historical scan.

# Wait for the pane to be discovered as managed (from the session_meta
# idle_heartbeat emitted by the codex-jsonl FSM Init state).
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 30

pass "Phase 0: Codex pane discovered as managed"

# Now inject task_started — the watcher is active and will read it.
log "Phase 1: injecting Codex task_started via JSONL"
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"compete-task-001"}}\n' >> "$REAL_JSONL"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

PROVIDER_P1=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Phase 1: provider is codex" "codex" "$PROVIDER_P1"

pass "Phase 1: Codex pane managed + running + deterministic (provider=codex)"

# ── Phase 2: Start Claude hooks → provider should switch to claude ────
# The claude_hooks source has rank 0 for Claude. When we inject fresh
# tool_start events, the resolver receives events from both Codex JSONL
# (still has the JSONL file) and Claude hooks (fresh injection).
#
# select_winning_provider: with 2 Det providers, the provider with the
# most recent non-heartbeat activity wins. Claude hook events are injected
# with Utc::now() timestamps, which are newer than the Codex JSONL events.

CLAUDE_SID="e2e-compete-claude-sess-$$"
CLAUDE_INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$CLAUDE_SID" "$PANE_ID")
log "Phase 2: Claude hook injector PID=$CLAUDE_INJECTOR_PID"

# Wait for provider to switch to claude.
# The resolver needs a few poll ticks to process the fresh Claude events
# and determine that Claude has more recent real activity than Codex.
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "claude" 30

PROVIDER_P2=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Phase 2: provider switched to claude" "claude" "$PROVIDER_P2"

# The pane should still be managed + deterministic
PRESENCE_P2=$(jq_get "$SOCKET" "$PANE_ID" "presence")
assert_eq "Phase 2: still managed" "managed" "$PRESENCE_P2"

EVIDENCE_P2=$(jq_get "$SOCKET" "$PANE_ID" "evidence_mode")
assert_eq "Phase 2: still deterministic" "deterministic" "$EVIDENCE_P2"

# Activity state should reflect Claude's tool_start → running
ACTIVITY_P2=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 2: activity_state=running (from Claude)" "running" "$ACTIVITY_P2"

pass "Phase 2: provider switched from codex to claude (Claude hooks won arbitration)"

# ── Phase 3: Stop Claude hooks, inject new Codex activity ─────────────
# When Claude hooks stop arriving, the Codex JSONL source (still reading
# the file) should become the sole deterministic source.
# Inject a new task_started in Codex JSONL to give Codex fresh activity.

kill "$CLAUDE_INJECTOR_PID" 2>/dev/null || true
CLAUDE_INJECTOR_PID=""
log "Phase 3: Claude hooks stopped, injecting new Codex activity via JSONL"

# Wait a couple of seconds for Claude events to go stale (resolver freshness check)
sleep 3

# Inject a fresh Codex event to re-assert Codex presence
printf '{"type":"event_msg","payload":{"type":"task_started","taskId":"compete-task-002"}}\n' >> "$REAL_JSONL"

# The provider should switch back to codex since Claude hooks are stale
# and Codex has fresh JSONL activity.
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "codex" 30

PROVIDER_P3=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Phase 3: provider switched back to codex" "codex" "$PROVIDER_P3"

pass "Phase 3: provider switched from claude back to codex (Codex JSONL won after Claude stopped)"

# ── Phase 4: Verify final state consistency ───────────────────────────
# The pane should be in a clean state: managed, deterministic, codex, running.

FINAL_PRESENCE=$(jq_get "$SOCKET" "$PANE_ID" "presence")
assert_eq "Phase 4: final presence" "managed" "$FINAL_PRESENCE"

FINAL_EVIDENCE=$(jq_get "$SOCKET" "$PANE_ID" "evidence_mode")
assert_eq "Phase 4: final evidence_mode" "deterministic" "$FINAL_EVIDENCE"

FINAL_ACTIVITY=$(jq_get "$SOCKET" "$PANE_ID" "activity_state")
assert_eq "Phase 4: final activity_state" "running" "$FINAL_ACTIVITY"

# Verify agtmux ls shows the correct provider label
LS_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" ls 2>/dev/null || echo "")
log "Phase 4: agtmux ls output: $LS_OUT"

# JSON ground truth
JSON_OUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null || echo '{"version":1,"panes":[]}')
FINAL_PROVIDER=$(echo "$JSON_OUT" | jq -r --arg p "$PANE_ID" '.panes[] | select(.pane_id==$p) | .provider')
assert_eq "Phase 4: JSON provider field" "codex" "$FINAL_PROVIDER"

pass "Phase 4: final state consistent (managed/deterministic/codex/running)"

echo "=== source-competition.sh PASS ==="
