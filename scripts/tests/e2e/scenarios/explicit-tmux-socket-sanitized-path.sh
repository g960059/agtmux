#!/usr/bin/env bash
# scenarios/explicit-tmux-socket-sanitized-path.sh
#
# Regression for T-XTERM-A6:
# daemon launched with explicit --tmux-socket must still inventory panes when PATH
# is stripped to the app/XCUITest-style baseline.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"

set -euo pipefail

TMUX_BIN="$(resolve_tmux_bin)"

SESSION="e2e-explicit-socket-$$"
SOCKET="/tmp/agtmux-e2e-explicit-socket-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-explicit-socket-workdir-$$"
TMUX_SOCKET_NAME="agtmux-e2e-explicit-socket-$$"
LOG_PATH="/tmp/agtmux-e2e-explicit-socket-$$.log"
DAEMON_PID=""

tmux() {
    "$TMUX_BIN" -L "$TMUX_SOCKET_NAME" "$@"
}

cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    tmux kill-server 2>/dev/null || true
    rm -rf "$(dirname "$SOCKET")" "$WORKDIR"
    rm -f "$LOG_PATH"
}
trap cleanup EXIT

mkdir -p "$WORKDIR" "$(dirname "$SOCKET")"
tmux new-session -d -s "$SESSION" -n main 'zsh -l' 2>/dev/null

PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)"
[ -n "$PANE_ID" ] || fail "could not resolve pane_id"
TMUX_SOCKET_PATH="$(tmux display-message -p '#{socket_path}' 2>/dev/null | head -1)"
[ -n "$TMUX_SOCKET_PATH" ] || fail "could not resolve tmux socket path"

log "launching daemon with explicit --tmux-socket under stripped PATH"
env PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
    "$AGTMUX_BIN" --socket-path "$SOCKET" daemon --tmux-socket "$TMUX_SOCKET_PATH" \
    >"$LOG_PATH" 2>&1 &
DAEMON_PID=$!

wait_for_socket "$SOCKET" 15
sleep 2

PRESENCE="$(jq_get "$SOCKET" "$PANE_ID" "presence")"
CURRENT_CMD="$(jq_get "$SOCKET" "$PANE_ID" "current_cmd")"

[ "$PRESENCE" != "null" ] || {
    cat "$LOG_PATH" >&2 || true
    fail "explicit --tmux-socket daemon returned no pane row for $PANE_ID under stripped PATH"
}

assert_eq "explicit-socket current_cmd" "zsh" "$CURRENT_CMD"
pass "explicit --tmux-socket inventory survives stripped PATH"
