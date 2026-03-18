#!/usr/bin/env bash
# scenarios/explicit-tmux-socket-shell-child-promotion.sh
#
# Regression for T-XTERM-A6:
# daemon launched under an app-like child-process env with explicit
# --tmux-socket must still promote a plain shell pane to managed when its
# child process tree clearly contains a real Claude/Codex run.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"

PROVIDER="${PROVIDER:-codex}"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

set -euo pipefail

TMUX_BIN="$(resolve_tmux_bin)"

if [[ "${AGTMUX_BIN:-}" != /* ]]; then
    REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || pwd)"
    if [ ! -x "$REPO_ROOT/target/debug/agtmux" ] && [ ! -x "$REPO_ROOT/target/release/agtmux" ]; then
        cargo build -p agtmux --quiet >/dev/null 2>&1
    fi
    if [ -x "$REPO_ROOT/target/debug/agtmux" ]; then
        AGTMUX_BIN="$REPO_ROOT/target/debug/agtmux"
    elif [ -x "$REPO_ROOT/target/release/agtmux" ]; then
        AGTMUX_BIN="$REPO_ROOT/target/release/agtmux"
    fi
fi
[ -x "$AGTMUX_BIN" ] || fail "agtmux binary must resolve to an absolute path for explicit env launch"

SESSION="e2e-explicit-shell-child-${PROVIDER}-$$"
WORKDIR="/tmp/agtmux-e2e-explicit-shell-child-${PROVIDER}-$$"
SOCKET="$WORKDIR/agtmuxd.sock"
TMUX_SOCKET_PATH="$WORKDIR/tmux.sock"
LOG_PATH="$WORKDIR/daemon.log"
PANE_ID=""
DAEMON_PID=""

echo "=== explicit-tmux-socket-shell-child-promotion.sh (PROVIDER=${PROVIDER}) ==="

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
    echo "--- provider: $PROVIDER ---" >&2
    echo "--- tmux socket: $TMUX_SOCKET_PATH ---" >&2
    ls -l "$TMUX_SOCKET_PATH" >&2 || true
    echo "--- tmux sessions ---" >&2
    tmux list-sessions 2>&1 >&2 || true
    echo "--- tmux panes ---" >&2
    tmux list-panes -a -F '#{session_name} #{window_name} #{pane_id} #{pane_pid} #{pane_current_command}' 2>&1 >&2 || true
    if [ -n "$PANE_ID" ]; then
        echo "--- pane capture ($PANE_ID) ---" >&2
        tmux capture-pane -p -t "$PANE_ID" 2>&1 >&2 || true
    fi
    echo "--- agtmux json ---" >&2
    "$AGTMUX_BIN" --socket-path "$SOCKET" json 2>&1 >&2 || true
    echo "--- daemon log ---" >&2
    cat "$LOG_PATH" >&2 || true
}

mkdir -p "$WORKDIR"
rm -f "$TMUX_SOCKET_PATH"

log "starting tmux session on explicit socket before daemon launch"
tmux new-session -d -s "$SESSION" -n main 'zsh -l' 2>/dev/null

for _ in $(seq 1 20); do
    PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1 || true)"
    [ -n "$PANE_ID" ] && break
    sleep 0.5
done
[ -n "$PANE_ID" ] || {
    dump_repro_context
    fail "could not resolve pane_id for explicit tmux socket shell-child scenario"
}

APP_PATH="${APP_PATH:-/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin}"
APP_HOME="${HOME}"
APP_USER="${USER:-$(id -un)}"
APP_LOGNAME="${LOGNAME:-$APP_USER}"
APP_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$APP_HOME/.config}"
APP_CODEX_HOME="${CODEX_HOME:-$APP_HOME/.codex}"

log "launching daemon with explicit --tmux-socket under app-like normalized env"
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
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "current_cmd" "zsh" 20 || {
    dump_repro_context
    fail "daemon never inventoried initial zsh pane $PANE_ID on explicit tmux socket"
}

TASK="Run exactly one bash command and do not run any additional commands. Wait 20 seconds by using sleep 20. bash -lc 'sleep 20; printf \"wait_result=managed\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=managed"

log "launching $PROVIDER from plain zsh pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"
wait_until_provider_running "$PANE_ID" 20 || log "WARN: provider-side running check timed out"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "$PROVIDER" 60 || {
    dump_repro_context
    fail "explicit tmux socket shell-child pane never promoted to provider=$PROVIDER"
}
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 60 || {
    dump_repro_context
    fail "explicit tmux socket shell-child pane never promoted to managed"
}

pass "explicit --tmux-socket shell child promotion reached managed/provider=$PROVIDER"

echo "=== explicit-tmux-socket-shell-child-promotion.sh PASS (PROVIDER=${PROVIDER}) ==="
