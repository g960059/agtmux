#!/usr/bin/env bash
# contract/test-freshness-fallback.sh — deterministic freshness timeout → heuristic fallback
#
# Verifies the DOWN_THRESHOLD (15s) mechanism:
#   When no deterministic events arrive for >15s, tick_freshness() downgrades
#   evidence_mode from "deterministic" to "heuristic".
#
# Resolver behaviour on stale det (resolver.rs Step 4):
#   Freshness::Stale | Freshness::Down => winner_tier = EvidenceTier::Heuristic
# So evidence_mode always becomes "heuristic" (never "none") after timeout.
# The pane remains "managed" — only evidence tier downgrades.
#
# NOTE: This test sleeps ~18s to outlast the 15s DOWN_THRESHOLD.
#       Expected total runtime: ~25s.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-freshness-$$"
SOCKET="/tmp/agtmux-e2e-freshness-$$/agtmuxd.sock"
THREAD_ID="e2e-fresh-thread-$$"
INJECTOR_PID=""

echo "=== test-freshness-fallback.sh ==="
echo "NOTE: this test deliberately waits >15s (DOWN_THRESHOLD)"

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane_id=$PANE_ID thread_id=$THREAD_ID"

cleanup_freshness() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_freshness EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Phase 1: Establish deterministic binding ────────────────────────────────
# Use lifecycle.running via codex_appserver (CodexRawEvent accepts any event_type string).

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.running" "$THREAD_ID" "$PANE_ID")
log "Running injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 15

pass "Phase 1: pane is managed / running / deterministic"

# ── Phase 2: Stop injection → wait for DOWN_THRESHOLD ──────────────────────
# DOWN_THRESHOLD = 15s (resolver.rs). tick_freshness() runs every poll tick (500ms).
# We wait 18s after last injection to guarantee the threshold has passed.

log "stopping injection — waiting 18s for DOWN_THRESHOLD (15s) to expire"
kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

# wait_for_agtmux_state polls every 1s; give it a generous window.
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "heuristic" 30

pass "Phase 2: evidence_mode downgraded to heuristic after deterministic timeout"

# ── Phase 3: Verify pane stays managed ────────────────────────────────────
# tick_freshness() only updates evidence_mode — presence is NOT cleared.

PRESENCE=$(jq_get "$SOCKET" "$PANE_ID" "presence")
assert_eq "pane still managed after fallback" "managed" "$PRESENCE"

pass "Phase 3: pane remains managed (evidence_mode only, not presence)"

# ── Phase 4: Re-establish deterministic ────────────────────────────────────
# Injecting new events should promote back to deterministic.

INJECTOR_PID=$(inject_codex_event_loop "$SOCKET" "lifecycle.running" "$THREAD_ID" "$PANE_ID")
log "re-injection PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 15

pass "Phase 4: re-injection promotes evidence_mode back to deterministic"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-freshness-fallback.sh PASS ==="
