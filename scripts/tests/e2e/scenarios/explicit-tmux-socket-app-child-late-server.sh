#!/usr/bin/env bash
# scenarios/explicit-tmux-socket-app-child-late-server.sh
#
# Regression for T-XTERM-A6 Phase 1:
# launch the daemon first under an app-like normalized child-process env with an
# explicit --tmux-socket, then start the tmux server/session afterwards. The
# daemon must eventually inventory the late-started pane on that socket.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"

set -euo pipefail

TMUX_BIN="$(resolve_tmux_bin)"

SESSION="e2e-explicit-late-server-$$"
WORKDIR="/tmp/agtmux-e2e-explicit-late-server-$$"
SOCKET="$WORKDIR/agtmuxd.sock"
TMUX_SOCKET_PATH="$WORKDIR/tmux.sock"
LOG_PATH="$WORKDIR/daemon.log"
DAEMON_PID=""

tmux() {
    env -u TMUX -u TMUX_PANE "$TMUX_BIN" -S "$TMUX_SOCKET_PATH" "$@"
}

cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    tmux kill-server 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

dump_repro_context() {
    echo "--- tmux socket: $TMUX_SOCKET_PATH ---" >&2
    ls -l "$TMUX_SOCKET_PATH" >&2 || true
    echo "--- tmux sessions ---" >&2
    tmux list-sessions 2>&1 >&2 || true
    echo "--- tmux panes ---" >&2
    tmux list-panes -a -F '#{session_name} #{window_name} #{pane_id} #{pane_current_command}' 2>&1 >&2 || true
    echo "--- daemon log ---" >&2
    cat "$LOG_PATH" >&2 || true
}

mkdir -p "$WORKDIR"
rm -f "$TMUX_SOCKET_PATH"

APP_PATH="${APP_PATH:-/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin}"
APP_HOME="${HOME}"
APP_USER="${USER:-$(id -un)}"
APP_LOGNAME="${LOGNAME:-$APP_USER}"
APP_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$APP_HOME/.config}"
APP_CODEX_HOME="${CODEX_HOME:-$APP_HOME/.codex}"

log "launching daemon first with explicit --tmux-socket under app-like normalized env"
env -i \
    HOME="$APP_HOME" \
    USER="$APP_USER" \
    LOGNAME="$APP_LOGNAME" \
    XDG_CONFIG_HOME="$APP_XDG_CONFIG_HOME" \
    CODEX_HOME="$APP_CODEX_HOME" \
    PATH="$APP_PATH" \
    TMUX_BIN="$TMUX_BIN" \
    "$AGTMUX_BIN" --socket-path "$SOCKET" daemon --tmux-socket "$TMUX_SOCKET_PATH" \
    >"$LOG_PATH" 2>&1 &
DAEMON_PID=$!

wait_for_socket "$SOCKET" 15

INITIAL_TOTAL="$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null | jq '.panes | length' 2>/dev/null || echo "unknown")"
log "initial agtmux json pane count before tmux server start: $INITIAL_TOTAL"

log "starting tmux server/session after daemon launch"
tmux new-session -d -s "$SESSION" -n main 'zsh -l' 2>/dev/null

PANE_ID=""
for _ in $(seq 1 20); do
    PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1 || true)"
    [ -n "$PANE_ID" ] && break
    sleep 0.5
done
[ -n "$PANE_ID" ] || {
    dump_repro_context
    fail "could not resolve pane_id for late-started tmux session"
}

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "current_cmd" "zsh" 20 || {
    dump_repro_context
    fail "daemon never inventoried late-started pane $PANE_ID on explicit tmux socket"
}

PRESENCE="$(jq_get "$SOCKET" "$PANE_ID" "presence")"
SESSION_NAME="$(jq_get "$SOCKET" "$PANE_ID" "session_name")"
CURRENT_CMD="$(jq_get "$SOCKET" "$PANE_ID" "current_cmd")"

assert_eq "late-server presence" "unmanaged" "$PRESENCE"
assert_eq "late-server session_name" "$SESSION" "$SESSION_NAME"
assert_eq "late-server current_cmd" "zsh" "$CURRENT_CMD"

pass "explicit --tmux-socket late-started tmux server is inventoried under app-like child env"
