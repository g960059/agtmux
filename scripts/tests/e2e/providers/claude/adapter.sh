#!/usr/bin/env bash
# providers/claude/adapter.sh — Claude Code provider adapter for Layer 3 Detection E2E
#
# Provides three functions that scenarios/*.sh call:
#   launch_provider    PANE_ID WORKDIR [TASK]
#   wait_until_provider_running  PANE_ID [TIMEOUT_S]
#   wait_until_provider_idle     PANE_ID [TIMEOUT_S]
#
# Detection mechanism: Claude hooks → source.ingest UDS events
# Fallback: JSONL source (~/.claude/projects/<encoded-path>/)

PROVIDER_NAME="claude"

# launch_provider PANE_ID WORKDIR [TASK]
# Sends Claude CLI command to the tmux pane via send-keys.
# Default task: read /etc/hosts and count lines → write result to WORKDIR/result.txt
launch_provider() {
    local pane_id="$1" workdir="$2"
    local task="${3:-count lines in /etc/hosts and write the count to result.txt}"
    tmux send-keys -t "$pane_id" \
        "cd $(printf '%q' "$workdir") && claude --dangerously-skip-permissions -p $(printf '%q' "$task")" \
        Enter
}

# wait_until_provider_running PANE_ID [TIMEOUT_S]
# Polls tmux capture-pane until Claude output is visible (tool use / thinking started).
# Returns 0 on success, 1 on timeout (non-fatal; agtmux state is authoritative).
wait_until_provider_running() {
    local pane_id="$1" timeout="${2:-60}"
    local elapsed=0
    while [ "$elapsed" -lt "$timeout" ]; do
        local capture
        capture=$(tmux capture-pane -t "$pane_id" -p 2>/dev/null | tail -10)
        # Claude shows these patterns when actively working
        if printf '%s\n' "$capture" | grep -qE '(Claude|●|Thinking|Tool|bash|Read|Write|counting)'; then
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done
    echo "[WARN] claude adapter: wait_until_provider_running timeout ${timeout}s for pane $pane_id" >&2
    return 1  # non-fatal; let wait_for_agtmux_state be authoritative
}

# wait_until_provider_idle PANE_ID [TIMEOUT_S]
# Polls tmux capture-pane until the shell prompt has returned (Claude done).
# Returns 0 on success, 1 on timeout.
wait_until_provider_idle() {
    local pane_id="$1" timeout="${2:-180}"
    local elapsed=0
    while [ "$elapsed" -lt "$timeout" ]; do
        local last_lines
        last_lines=$(tmux capture-pane -t "$pane_id" -p 2>/dev/null | tail -5)
        # Shell prompt returned ($ or % at end of line, possibly with spaces)
        if printf '%s\n' "$last_lines" | grep -qE '(\$|%) *$'; then
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done
    echo "[WARN] claude adapter: wait_until_provider_idle timeout ${timeout}s for pane $pane_id" >&2
    return 1
}
