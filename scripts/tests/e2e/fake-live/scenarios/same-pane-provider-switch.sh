#!/usr/bin/env bash
# fake-live/scenarios/same-pane-provider-switch.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../common.sh"

TMUX_SOCKET_NAME="agtmux-fake-live-switch-$$"
SESSION="e2e-fake-live-switch-$$"
WORKDIR="/tmp/agtmux-fake-live-switch-$$"
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

echo "=== same-pane-provider-switch.sh ==="

prepare_fake_live_artifacts "same-pane-provider-switch"
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
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 30

wait_for_shell_demotion "$SOCKET" "$PANE_ID" 45
assert_eq "claude demotion" "unmanaged" "$(jq_get "$SOCKET" "$PANE_ID" "presence")"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "null" 20

launch_fake_codex "$PANE_ID" "$WORKDIR" "slow_complete"
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "codex" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "running" 45
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "evidence_mode" "deterministic" 45

wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 60 >/dev/null
pass "same pane switched from fake claude to fake codex without stale carryover"
