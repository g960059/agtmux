#!/usr/bin/env bash
# fake-live/scenarios/shell-demotion-same-pane.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../common.sh"

TMUX_SOCKET_NAME="agtmux-fake-live-demotion-$$"
SESSION="e2e-fake-live-demotion-$$"
WORKDIR="/tmp/agtmux-fake-live-demotion-$$"
SOCKET="$WORKDIR/agtmuxd.sock"

tmux() {
    env -u TMUX -u TMUX_PANE command tmux -L "$TMUX_SOCKET_NAME" "$@"
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

echo "=== shell-demotion-same-pane.sh ==="

prepare_fake_live_artifacts "shell-demotion-same-pane"
mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main
PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' | head -1)"
[ -n "$PANE_ID" ] || fail "could not resolve pane for $SESSION"
remember_fake_live_context "$SOCKET" "$SESSION" "$PANE_ID"

daemon_start "$SOCKET" 500

launch_fake_claude "$PANE_ID" "$WORKDIR" "$SOCKET" "running_then_stop"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "claude" 30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 30

wait_for_shell_demotion "$SOCKET" "$PANE_ID" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "null" 20
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "unmanaged" 20

current_cmd="$(jq_get "$SOCKET" "$PANE_ID" "current_cmd")"
is_shell_cmd "$current_cmd" || fail "expected shell current_cmd after demotion, got '$current_cmd'"
pass "managed fake claude pane demoted back to exact-row shell truth"
