#!/usr/bin/env bash
# fake-live/scenarios/daemon-restart-rebind.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../common.sh"

TMUX_BIN="$(resolve_tmux_bin)"
TMUX_SOCKET_NAME="agtmux-fake-live-restart-$$"
SESSION="e2e-fake-live-restart-$$"
WORKDIR="/tmp/agtmux-fake-live-restart-$$"
SOCKET="$WORKDIR/agtmuxd.sock"

tmux() {
    env -u TMUX -u TMUX_PANE "$TMUX_BIN" -L "$TMUX_SOCKET_NAME" "$@"
}

export AGTMUX_TMUX_SOCKET_NAME="$TMUX_SOCKET_NAME"

PANE_ID=""

cleanup() {
    local exit_code="$?"
    if [ "$exit_code" -ne 0 ]; then
        dump_fake_live_artifacts "$SOCKET" "$SESSION" "$PANE_ID"
    fi
    daemon_stop
    tmux kill-server 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

echo "=== daemon-restart-rebind.sh ==="

prepare_fake_live_artifacts "daemon-restart-rebind"
mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main -c "$WORKDIR"
PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' | head -1)"
[ -n "$PANE_ID" ] || fail "could not resolve pane for $SESSION"
remember_fake_live_context "$SOCKET" "$SESSION" "$PANE_ID"

daemon_start "$SOCKET" 500

launch_fake_codex "$PANE_ID" "$WORKDIR" "long_running"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "codex" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 45

count_before="$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null | jq -r --arg pane "$PANE_ID" '[.panes[] | select(.pane_id == $pane)] | length')"
assert_eq "single row before restart" "1" "$count_before"

daemon_stop
sleep 1
daemon_start "$SOCKET" 500

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "codex" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 45

count_after="$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null | jq -r --arg pane "$PANE_ID" '[.panes[] | select(.pane_id == $pane)] | length')"
assert_eq "single row after restart" "1" "$count_after"

wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 90 >/dev/null
pass "daemon restart rebound to the same fake codex pane without duplicates"
