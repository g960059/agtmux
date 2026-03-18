#!/usr/bin/env bash
# fake-live/common.sh — shared helpers for fake-live tmux integration tests

set -euo pipefail

FAKE_LIVE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$FAKE_LIVE_DIR/../harness/common.sh"
source "$FAKE_LIVE_DIR/../harness/daemon.sh"

FAKE_LIVE_AGENT_DIR="$FAKE_LIVE_DIR/agents"
FAKE_LIVE_REPO_ROOT="$(git -C "$FAKE_LIVE_DIR" rev-parse --show-toplevel 2>/dev/null || pwd)"
FAKE_LIVE_ARTIFACT_ROOT="${FAKE_LIVE_ARTIFACT_ROOT:-$FAKE_LIVE_REPO_ROOT/target/e2e-artifacts/fake-live}"

FAKE_LIVE_SCENARIO_NAME=""
FAKE_LIVE_SCENARIO_ARTIFACTS=""
FAKE_LIVE_SOCKET=""
FAKE_LIVE_SESSION=""
FAKE_LIVE_PANES=()

prepare_fake_live_artifacts() {
    local scenario_name="$1"
    FAKE_LIVE_SCENARIO_NAME="$scenario_name"
    FAKE_LIVE_SCENARIO_ARTIFACTS="$FAKE_LIVE_ARTIFACT_ROOT/$scenario_name"
    mkdir -p "$FAKE_LIVE_SCENARIO_ARTIFACTS"
}

remember_fake_live_context() {
    FAKE_LIVE_SOCKET="$1"
    FAKE_LIVE_SESSION="$2"
    shift 2
    FAKE_LIVE_PANES=("$@")
}

sanitize_component() {
    printf '%s' "$1" | tr -c '[:alnum:]._-' '_'
}

copy_codex_spool_artifacts() {
    local socket_component=""
    local spool_root="$HOME/.agtmux/codex-exec-spool"
    if [ -n "${AGTMUX_TMUX_SOCKET_PATH:-}" ]; then
        socket_component="$(sanitize_component "$AGTMUX_TMUX_SOCKET_PATH")"
    elif [ -n "${AGTMUX_TMUX_SOCKET_NAME:-}" ]; then
        socket_component="$(sanitize_component "$AGTMUX_TMUX_SOCKET_NAME")"
    else
        socket_component="default"
    fi

    if [ -d "$spool_root/$socket_component" ]; then
        cp -R "$spool_root/$socket_component" "$FAKE_LIVE_SCENARIO_ARTIFACTS/codex-exec-spool"
    fi
}

dump_fake_live_artifacts() {
    local socket="${1:-$FAKE_LIVE_SOCKET}"
    local session="${2:-$FAKE_LIVE_SESSION}"
    shift 2 || true
    local panes=()
    local pane=""
    local index=0

    if [ "$#" -gt 0 ]; then
        panes=("$@")
    else
        panes=("${FAKE_LIVE_PANES[@]:-}")
    fi

    [ -n "$FAKE_LIVE_SCENARIO_ARTIFACTS" ] || return 0
    mkdir -p "$FAKE_LIVE_SCENARIO_ARTIFACTS"

    if [ -n "$socket" ] && [ -S "$socket" ]; then
        "$AGTMUX_BIN" --socket-path "$socket" json \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/agtmux.json" 2>&1 || true
        "$AGTMUX_BIN" --socket-path "$socket" json --health \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/agtmux-health.json" 2>&1 || true
        daemon_rpc "$socket" "list_panes" \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/daemon-list-panes.json" 2>&1 || true
    fi

    if [ -n "$session" ]; then
        tmux list-sessions >"$FAKE_LIVE_SCENARIO_ARTIFACTS/tmux-sessions.txt" 2>&1 || true
        tmux list-panes -t "$session" -F '#{session_name} #{window_name} #{pane_id} #{pane_pid} #{pane_current_command}' \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/tmux-panes.txt" 2>&1 || true
    fi

    for pane in "${panes[@]}"; do
        [ -n "$pane" ] || continue
        tmux capture-pane -t "$pane" -p -S -200 \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/pane-${index}.txt" 2>&1 || true
        tmux display-message -p -t "$pane" '#{pane_id} #{pane_pid} #{pane_tty} #{pane_current_command}' \
            >"$FAKE_LIVE_SCENARIO_ARTIFACTS/pane-${index}.meta.txt" 2>&1 || true
        index=$((index + 1))
    done

    if [ -f "/tmp/agtmux-e2e-daemon-$$.log" ]; then
        cp "/tmp/agtmux-e2e-daemon-$$.log" "$FAKE_LIVE_SCENARIO_ARTIFACTS/daemon.log"
    fi

    copy_codex_spool_artifacts || true
}

wait_for_shell_demotion() {
    local socket="$1" pane_id="$2"
    local timeout="${3:-45}"
    local elapsed=0 presence="" provider="" activity="" current_cmd=""

    while [ "$elapsed" -lt "$timeout" ]; do
        presence="$(jq_get "$socket" "$pane_id" "presence")"
        provider="$(jq_get "$socket" "$pane_id" "provider")"
        activity="$(jq_get "$socket" "$pane_id" "activity_state")"
        current_cmd="$(jq_get "$socket" "$pane_id" "current_cmd")"
        if [ "$presence" = "unmanaged" ] \
            && [ "$provider" = "null" ] \
            && [ "$activity" = "null" ] \
            && is_shell_cmd "$current_cmd"; then
            log "wait_for_shell_demotion OK: pane=$pane_id (${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    fail "timeout waiting for shell demotion pane=$pane_id presence=$presence provider=$provider activity=$activity current_cmd=$current_cmd"
}

launch_fake_claude() {
    local pane_id="$1" workdir="$2" socket="$3"
    local mode="${4:-running_then_stop}"
    local session_id="fake-claude-$(printf '%s' "$pane_id" | tr -cd '[:alnum:]')-${mode}-$$"

    tmux send-keys -t "$pane_id" \
        "cd $(printf '%q' "$workdir") && bash $(printf '%q' "$FAKE_LIVE_AGENT_DIR/fake-claude.sh") $(printf '%q' "$socket") $(printf '%q' "$pane_id") $(printf '%q' "$mode") $(printf '%q' "$session_id")" \
        Enter
}

launch_fake_codex() {
    local pane_id="$1" workdir="$2"
    local profile="${3:-slow_complete}"

    tmux send-keys -t "$pane_id" \
        "cd $(printf '%q' "$workdir") && bash $(printf '%q' "$FAKE_LIVE_AGENT_DIR/fake-codex.sh") $(printf '%q' "$profile")" \
        Enter
}

launch_raw_shell() {
    local pane_id="$1" workdir="$2"
    local profile="${3:-default}"
    tmux send-keys -t "$pane_id" \
        "cd $(printf '%q' "$workdir") && bash $(printf '%q' "$FAKE_LIVE_AGENT_DIR/raw-shell.sh") $(printf '%q' "$profile")" \
        Enter
}
