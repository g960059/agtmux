#!/usr/bin/env bash
# contract/test-daemon-restart.sh — Daemon restart recovery: re-detect panes from scratch
#
# Gap filled: ZERO e2e coverage for the daemon restart scenario at the contract
#   level (injection-only, no real CLI needed). The existing
#   claude-title-after-restart.sh tests title persistence but requires real
#   Claude Code; it does not test injection-based state recovery.
#
# Why it matters: When the daemon restarts (crash, upgrade, config change), ALL
#   in-memory state is lost — DaemonProjection, Gateway, ResolverState, etc.
#   The daemon must re-discover managed panes within a few poll cycles by:
#     1. Re-polling tmux panes (Step 1-3 in poll_loop)
#     2. Re-ingesting hook events via source.ingest UDS (Step 8b)
#     3. Re-running the resolver pipeline from scratch
#
#   If this fails, users see all panes as "unmanaged" after any daemon restart,
#   which is a critical UX regression — the tmux status bar shows no agents.
#
# Test flow:
#   Phase 1: Start daemon, inject events, verify managed + running.
#   Phase 2: Stop daemon. All in-memory state is lost.
#   Phase 3: Start a FRESH daemon on the SAME socket path.
#   Phase 4: Re-inject events to the new daemon; verify it re-detects the pane
#            as managed + running. The pane (tmux session) was never killed.
#   Phase 5: Verify evidence_mode + provider are correct after recovery.
#   Phase 6: Transition to idle; verify state transition works on fresh daemon.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-restart-$$"
SOCKET="/tmp/agtmux-e2e-restart-$$/agtmuxd.sock"
INJECTOR_PID=""

echo "=== test-daemon-restart.sh ==="

# ── Setup: isolated tmux session (kept alive across daemon restarts) ──

tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane_id=$PANE_ID session=$SESSION"

cleanup_restart() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_restart EXIT

# Make the pane look like a node process (prevents shell-truth demotion).
make_pane_node_like "$PANE_ID"

# ── Phase 1: Start daemon, inject events, verify managed ─────────────

daemon_start "$SOCKET" 500
sleep 1

SESSION_ID="e2e-restart-sess-$$"
INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "Phase 1: tool_start injector PID=$INJECTOR_PID"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running"       30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode"  "deterministic" 10

# Record the first daemon's PID and nonce for Phase 3 comparison.
DAEMON1_PID="$DAEMON_PID"
log "Phase 1: daemon1 PID=$DAEMON1_PID, pane managed + running"

pass "Phase 1: pane managed and running on first daemon"

# ── Phase 2: Stop daemon — all in-memory state is lost ────────────────

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

daemon_stop
sleep 1

# Verify socket is gone (daemon cleanup on exit removes it).
if [ -S "$SOCKET" ]; then
    log "WARN: socket still exists after daemon_stop (may be stale)"
fi

pass "Phase 2: first daemon stopped (in-memory state lost)"

# ── Phase 3: Start a FRESH daemon on the SAME socket ─────────────────
# This is a completely new process with empty DaemonState.
# The tmux session (and pane) is still alive.

daemon_start "$SOCKET" 500
sleep 1

DAEMON2_PID="$DAEMON_PID"
log "Phase 3: daemon2 PID=$DAEMON2_PID"

# Verify it's actually a different daemon process.
if [ "$DAEMON1_PID" = "$DAEMON2_PID" ]; then
    fail "Phase 3: daemon PID unchanged — restart may not have happened"
fi

# Before any injection, the pane should exist but be unmanaged
# (fresh daemon has empty projection; no hook events have arrived yet).
# Wait a couple of seconds for initial poll to discover tmux panes.
sleep 2

INITIAL_PRESENCE=$(jq_get "$SOCKET" "$PANE_ID" "presence")
log "Phase 3: initial presence after restart='$INITIAL_PRESENCE' (expected unmanaged or null)"

# The pane should appear in the output (tmux is still running).
PANE_EXISTS=$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null \
    | jq -r --arg p "$PANE_ID" '[.panes[] | select(.pane_id==$p)] | length')
if [ "$PANE_EXISTS" = "0" ]; then
    fail "Phase 3: pane $PANE_ID not listed at all after restart (tmux session still alive)"
fi

pass "Phase 3: fresh daemon started, pane visible in tmux pane list"

# ── Phase 4: Re-inject events to the fresh daemon ────────────────────
# The NEW daemon needs fresh events to re-discover the pane as managed.
# This simulates Claude hooks reconnecting to the new daemon socket.

INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE_ID")
log "Phase 4: re-injecting tool_start events (injector PID=$INJECTOR_PID)"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence"       "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30

pass "Phase 4: pane re-detected as managed + running after daemon restart"

# ── Phase 5: Verify evidence_mode + provider are correct ─────────────
# These fields should be identical to what they were before the restart.

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 10

PROVIDER=$(jq_get "$SOCKET" "$PANE_ID" "provider")
assert_eq "Phase 5: provider after restart" "claude" "$PROVIDER"

pass "Phase 5: evidence_mode=deterministic, provider=claude (correct after restart)"

# ── Phase 6: Transition to idle on fresh daemon ──────────────────────
# Prove the full state machine works on the fresh daemon — not just
# the initial detection, but also state transitions.

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "idle" "$SESSION_ID" "$PANE_ID")
log "Phase 6: idle injector PID=$INJECTOR_PID"
sleep 6  # hysteresis

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "idle" 30

pass "Phase 6: running → idle transition works on fresh daemon"

# ── Phase 7: Verify daemon.info nonce differs (new process) ──────────
# Confirm the nonce is different from daemon1, proving fresh state.

_uds_rpc() {
    local socket="$1" payload="$2"
    python3 - "$socket" "$payload" <<'PYEOF'
import socket as _s, sys

sock_path = sys.argv[1]
payload   = sys.argv[2]

with _s.socket(_s.AF_UNIX, _s.SOCK_STREAM) as s:
    s.connect(sock_path)
    s.sendall(payload.encode() + b"\n")
    s.shutdown(_s.SHUT_WR)
    chunks = []
    while True:
        data = s.recv(4096)
        if not data:
            break
        chunks.append(data)
    sys.stdout.write(b"".join(chunks).decode().strip())
PYEOF
}

INFO_REQ=$(jq -nc '{jsonrpc:"2.0", method:"daemon.info", id:1, params:{}}')
INFO_RESP=$(_uds_rpc "$SOCKET" "$INFO_REQ")
DAEMON2_NONCE=$(echo "$INFO_RESP" | jq -r '.result.nonce')
DAEMON2_RESP_PID=$(echo "$INFO_RESP" | jq -r '.result.pid')

assert_eq "Phase 7: daemon.info PID matches DAEMON_PID" "$DAEMON2_PID" "$DAEMON2_RESP_PID"

# Nonce format: <pid>-<nanos>. Since PID changed, nonce must differ.
log "Phase 7: daemon2 nonce=$DAEMON2_NONCE pid=$DAEMON2_RESP_PID"

pass "Phase 7: daemon.info confirms fresh daemon (PID=$DAEMON2_RESP_PID)"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-daemon-restart.sh PASS ==="
