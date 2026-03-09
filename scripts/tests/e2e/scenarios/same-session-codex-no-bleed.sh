#!/usr/bin/env bash
# scenarios/same-session-codex-no-bleed.sh — exact-row isolation for same-session Codex panes
#
# Verifies:
#   - two Codex panes in the same tmux session/shared CWD can both be Running
#   - after one pane is forced back to shell, the sibling's Running state does not re-promote it

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

PROVIDER="${PROVIDER:-codex}"
[ "$PROVIDER" = "codex" ] || fail "same-session-codex-no-bleed.sh is codex-specific"
source "$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"

register_cleanup

SESSION="e2e-codex-nobleed-$$"
SOCKET="/tmp/agtmux-e2e-codex-nobleed-$$/agtmuxd.sock"
SHARED_CWD="/tmp/e2e-codex-nobleed-$$"
TMUX_SOCKET_NAME="agtmux-e2e-codex-nobleed-$$"

echo "=== same-session-codex-no-bleed.sh ==="

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

assert_demoted_without_running_bleed() {
    local socket="$1" demoted_pane="$2" running_pane="$3" max_wait="${4:-12}"
    local elapsed=0 presence="" provider="" activity="" sibling_activity=""

    while [ "$elapsed" -lt "$max_wait" ]; do
        presence=$(jq_get "$socket" "$demoted_pane" "presence")
        provider=$(jq_get "$socket" "$demoted_pane" "provider")
        activity=$(jq_get "$socket" "$demoted_pane" "activity_state")
        sibling_activity=$(jq_get "$socket" "$running_pane" "activity_state")

        [ "$sibling_activity" = "running" ] || fail "sibling pane=$running_pane stopped running before no-bleed window completed (activity=$sibling_activity)"
        [ "$presence" = "unmanaged" ] || fail "demoted pane=$demoted_pane re-promoted to presence=$presence while sibling remained running"
        [ "$provider" = "null" ] || fail "demoted pane=$demoted_pane regained provider=$provider while sibling remained running"
        [ "$activity" = "null" ] || fail "demoted pane=$demoted_pane regained activity_state=$activity while sibling remained running"

        sleep 1
        elapsed=$((elapsed + 1))
    done

    pass "pane $demoted_pane stayed demoted for ${max_wait}s while sibling pane $running_pane remained running"
}

TASK1="Run exactly one bash command and do not run any additional commands. Wait 30 seconds by using sleep 30. bash -lc 'sleep 30; printf \"wait_result=first\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=first"
TASK2="Run exactly one bash command and do not run any additional commands. Wait 30 seconds by using sleep 30. bash -lc 'sleep 30; printf \"wait_result=second\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=second"

mkdir -p "$SHARED_CWD"
tmux new-session -d -s "$SESSION" -n main 2>/dev/null
tmux split-window -h -t "$SESSION:main" 2>/dev/null

PANE_IDS=( $(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null) )
[ ${#PANE_IDS[@]} -ge 2 ] || fail "need at least 2 panes in session $SESSION"
PANE1="${PANE_IDS[0]}"
PANE2="${PANE_IDS[1]}"
log "pane1=$PANE1 pane2=$PANE2 session=$SESSION shared_cwd=$SHARED_CWD"
SHELL_CMD1=$(tmux display-message -p -t "$PANE1" '#{pane_current_command}' 2>/dev/null || true)
[ -n "$SHELL_CMD1" ] || fail "could not resolve initial shell command for $PANE1"

cleanup_no_bleed() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    tmux kill-server 2>/dev/null || true
    daemon_stop
    rm -rf "$SHARED_CWD"
}
trap cleanup_no_bleed EXIT

export AGTMUX_TMUX_SOCKET_NAME="$TMUX_SOCKET_NAME"
daemon_start "$SOCKET" 500
sleep 1

log "launching first codex pane=$PANE1"
launch_provider "$PANE1" "$SHARED_CWD" "$TASK1"
wait_for_agtmux_state "$SOCKET" "$PANE1" "provider" "codex" 60
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "running" 45
wait_for_pane_child_process "$PANE1" 45

log "launching sibling codex pane=$PANE2"
launch_provider "$PANE2" "$SHARED_CWD" "$TASK2"

wait_for_agtmux_state "$SOCKET" "$PANE2" "provider" "codex" 60
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running" 45
wait_for_agtmux_state "$SOCKET" "$PANE2" "presence" "managed" 60

pass "both codex panes reached running in the same session/shared CWD"

kill_pane_children "$PANE1"
wait_for_tmux_current_cmd "$PANE1" "$SHELL_CMD1" 20
wait_for_agtmux_state "$SOCKET" "$PANE1" "current_cmd" "$SHELL_CMD1" 20
wait_for_agtmux_state "$SOCKET" "$PANE1" "presence" "unmanaged" 20
wait_for_agtmux_state "$SOCKET" "$PANE1" "provider" "null" 20
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "null" 20
wait_for_agtmux_state "$SOCKET" "$PANE2" "activity_state" "running" 20

assert_demoted_without_running_bleed "$SOCKET" "$PANE1" "$PANE2" 12

echo "=== same-session-codex-no-bleed.sh PASS ==="
