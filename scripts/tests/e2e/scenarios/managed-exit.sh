#!/usr/bin/env bash
# scenarios/managed-exit.sh — exact-row demotion after agent exit / return-to-shell
#
# The original confirmed repro was Codex. This scenario is provider-generic so
# follow-up validation can confirm the same shell demotion contract for Claude.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER="${PROVIDER:-codex}"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

register_cleanup

SESSION="e2e-managed-exit-$$"
SOCKET="/tmp/agtmux-e2e-managed-exit-$$/agtmuxd.sock"
WORKDIR="/tmp/e2e-managed-exit-workdir-$$"
TMUX_SOCKET_NAME="agtmux-e2e-managed-exit-$$"

echo "=== managed-exit.sh (PROVIDER=${PROVIDER}) ==="

tmux() {
    command tmux -L "$TMUX_SOCKET_NAME" "$@"
}

wait_for_tmux_current_cmd() {
    local pane_id="$1" expected="$2" timeout="${3:-30}"
    local elapsed=0 actual=""

    while [ "$elapsed" -lt "$timeout" ]; do
        actual=$(tmux display-message -p -t "$pane_id" '#{pane_current_command}' 2>/dev/null || true)
        if [ "$actual" = "$expected" ]; then
            log "wait_for_tmux_current_cmd OK: pane=$pane_id cmd='$expected' (${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    fail "timeout(${timeout}s): pane=$pane_id expected tmux current command '$expected' got '$actual'"
}

wait_for_pane_child_process() {
    local pane_id="$1" timeout="${2:-30}"
    local elapsed=0 shell_pid="" child_pids=""

    while [ "$elapsed" -lt "$timeout" ]; do
        shell_pid=$(tmux display-message -p -t "$pane_id" '#{pane_pid}' 2>/dev/null || true)
        if [ -n "$shell_pid" ]; then
            child_pids=$(pgrep -P "$shell_pid" 2>/dev/null || true)
            if [ -n "$child_pids" ]; then
                log "wait_for_pane_child_process OK: pane=$pane_id shell_pid=$shell_pid children=$child_pids (${elapsed}s)"
                return 0
            fi
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    fail "timeout(${timeout}s): no child process found under pane shell for $pane_id"
}

kill_pane_children() {
    local pane_id="$1"
    local shell_pid child_pids

    shell_pid=$(tmux display-message -p -t "$pane_id" '#{pane_pid}' 2>/dev/null || true)
    [ -n "$shell_pid" ] || fail "could not resolve pane_pid for $pane_id"

    child_pids=$(pgrep -P "$shell_pid" 2>/dev/null || true)
    [ -n "$child_pids" ] || fail "no child processes found under pane shell pid=$shell_pid"

    log "force-terminating pane child processes under shell pid=$shell_pid: $child_pids"
    for pid in $child_pids; do
        pkill -TERM -P "$pid" 2>/dev/null || true
        kill -TERM "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in $child_pids; do
        pkill -KILL -P "$pid" 2>/dev/null || true
        kill -KILL "$pid" 2>/dev/null || true
    done
}

mkdir -p "$WORKDIR"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null

PANE_ID=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
[ -n "$PANE_ID" ] || fail "could not get pane_id from tmux session $SESSION"
log "using pane=$PANE_ID session=$SESSION"
SHELL_CMD=$(tmux display-message -p -t "$PANE_ID" '#{pane_current_command}' 2>/dev/null || true)
[ -n "$SHELL_CMD" ] || fail "could not resolve initial pane shell command for $PANE_ID"
log "initial shell command for pane $PANE_ID is '$SHELL_CMD'"

cleanup_managed_exit() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    tmux kill-server 2>/dev/null || true
    daemon_stop
    rm -rf "$WORKDIR"
}
trap cleanup_managed_exit EXIT

export AGTMUX_TMUX_SOCKET_NAME="$TMUX_SOCKET_NAME"
daemon_start "$SOCKET" 500
sleep 1

TASK="Run exactly one bash command and do not run any additional commands. Wait 60 seconds by using sleep 60. bash -lc 'sleep 60; printf \"wait_result=idle\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=idle"

log "launching $PROVIDER in pane $PANE_ID (workdir=$WORKDIR)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "$PROVIDER" 60
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "managed" 60
wait_until_provider_running "$PANE_ID" 15 || true
wait_for_pane_child_process "$PANE_ID" 45

pass "Phase 1: $PROVIDER pane entered managed state with a live child process"

kill_pane_children "$PANE_ID"
wait_for_tmux_current_cmd "$PANE_ID" "$SHELL_CMD" 20
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "current_cmd" "$SHELL_CMD" 20

pass "Phase 2: exact pane row returned to shell truth"

wait_for_agtmux_state "$SOCKET" "$PANE_ID" "presence" "unmanaged" 20
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "provider" "null" 20
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "activity_state" "null" 20

pass "Phase 3: agtmux demoted the exact row back to unmanaged shell truth"

echo "=== managed-exit.sh PASS (PROVIDER=${PROVIDER}) ==="
