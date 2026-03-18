#!/usr/bin/env bash
# fake-live/scenarios/same-cwd-multi-pane-no-bleed.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../common.sh"

TMUX_SOCKET_NAME="agtmux-fake-live-nobleed-$$"
SESSION="e2e-fake-live-nobleed-$$"
WORKDIR="/tmp/agtmux-fake-live-nobleed-$$"
SOCKET="$WORKDIR/agtmuxd.sock"

tmux() {
    env -u TMUX -u TMUX_PANE command tmux -L "$TMUX_SOCKET_NAME" "$@"
}

export AGTMUX_TMUX_SOCKET_NAME="$TMUX_SOCKET_NAME"

PANE_A=""
PANE_B=""

cleanup() {
    local exit_code="$?"
    if [ "$exit_code" -ne 0 ]; then
        dump_fake_live_artifacts "$SOCKET" "$SESSION" "$PANE_A" "$PANE_B"
    fi
    daemon_stop
    tmux kill-server 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

echo "=== same-cwd-multi-pane-no-bleed.sh ==="

prepare_fake_live_artifacts "same-cwd-multi-pane-no-bleed"
mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main -c "$WORKDIR"
tmux split-window -h -t "$SESSION:main" -c "$WORKDIR"

PANE_A="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' | sed -n '1p')"
PANE_B="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' | sed -n '2p')"
[ -n "$PANE_A" ] || fail "missing first pane"
[ -n "$PANE_B" ] || fail "missing second pane"
remember_fake_live_context "$SOCKET" "$SESSION" "$PANE_A" "$PANE_B"

daemon_start "$SOCKET" 500

launch_fake_claude "$PANE_A" "$WORKDIR" "$SOCKET" "running_then_stop"
launch_fake_codex "$PANE_B" "$WORKDIR" "long_running"

wait_for_agtmux_state "$SOCKET" "$PANE_A" "provider" "claude" 30
wait_for_agtmux_state "$SOCKET" "$PANE_A" "presence" "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE_A" "activity_state" "running" 30

wait_for_agtmux_state "$SOCKET" "$PANE_B" "provider" "codex" 45
wait_for_agtmux_state "$SOCKET" "$PANE_B" "presence" "managed" 45
wait_for_agtmux_state "$SOCKET" "$PANE_B" "activity_state" "running" 45

wait_for_shell_demotion "$SOCKET" "$PANE_A" 45
assert_eq "pane A demotion" "unmanaged" "$(jq_get "$SOCKET" "$PANE_A" "presence")"
wait_for_agtmux_state "$SOCKET" "$PANE_B" "provider" "codex" 20
wait_for_agtmux_state "$SOCKET" "$PANE_B" "presence" "managed" 20
wait_for_agtmux_state "$SOCKET" "$PANE_B" "activity_state" "running" 20

wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_B" 60 >/dev/null
pass "same-cwd panes stayed isolated across fake claude and fake codex activity"
