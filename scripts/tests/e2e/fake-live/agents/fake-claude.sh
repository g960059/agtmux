#!/usr/bin/env bash
# fake-live/agents/fake-claude.sh — deterministic Claude hook driver

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

SOCKET="$1"
PANE_ID="$2"
MODE="${3:-running_then_stop}"
SESSION_ID="${4:-fake-claude-$PANE_ID-$$}"

case "$MODE" in
    running_then_stop)
        printf 'Claude is thinking\n'
        send_claude_hook "$SOCKET" "$PANE_ID" "UserPromptSubmit" "$SESSION_ID" "" "bash"
        sleep_with_jitter 1
        send_claude_hook "$SOCKET" "$PANE_ID" "tool_start" "$SESSION_ID" "" "bash"
        printf 'Tool: bash\n'
        sleep_with_jitter 1
        send_claude_hook "$SOCKET" "$PANE_ID" "tool_start" "$SESSION_ID" "" "bash"
        printf 'Writing result\n'
        sleep_with_jitter 1
        send_claude_hook "$SOCKET" "$PANE_ID" "Stop" "$SESSION_ID"
        printf 'Claude finished\n'
        ;;
    approval_then_stop)
        printf 'Claude needs approval\n'
        send_claude_hook "$SOCKET" "$PANE_ID" "Notification" "$SESSION_ID" "permission_prompt"
        sleep_with_jitter 1
        send_claude_hook "$SOCKET" "$PANE_ID" "Notification" "$SESSION_ID" "permission_prompt"
        sleep_with_jitter 1
        send_claude_hook "$SOCKET" "$PANE_ID" "Stop" "$SESSION_ID"
        printf 'Claude stopped\n'
        ;;
    *)
        echo "[fake-claude] unknown mode: $MODE" >&2
        exit 1
        ;;
esac
