#!/usr/bin/env bash
# agtmux-claude-hook.sh — Claude Code hook adapter
#
# Reads hook JSON from stdin, wraps it as a source.ingest JSON-RPC request,
# and sends it to the agtmux daemon over UDS.
#
# Environment:
#   AGTMUX_HOOK_TYPE  — hook type (e.g. PreToolUse, PostToolUse, Notification, Stop)
#   TMUX_PANE         — tmux pane ID (set by tmux)
#   AGTMUX_SOCKET     — (optional) UDS path override
#
# Dependencies: jq, socat (or nc with unix socket support)
# Fire-and-forget: failures are silently ignored so Claude Code is never blocked.

set -euo pipefail

# Determine socket path
if [ -n "${AGTMUX_SOCKET:-}" ]; then
    SOCKET="$AGTMUX_SOCKET"
elif [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    SOCKET="${XDG_RUNTIME_DIR}/agtmux/agtmuxd.sock"
else
    SOCKET="/tmp/agtmux-${USER:-unknown}/agtmuxd.sock"
fi

# Read stdin (hook JSON payload)
INPUT=$(cat)

# Extract session_id from hook payload (Claude Code provides this)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // "unknown"' 2>/dev/null || echo "unknown")

# Generate a unique hook ID
HOOK_ID="hook-$(date +%s%N)-$$"

# Get hook type from environment
HOOK_TYPE="${AGTMUX_HOOK_TYPE:-unknown}"

# Get pane ID from tmux environment
PANE_ID="${TMUX_PANE:-}"

# Build the JSON-RPC request
REQUEST=$(jq -n \
    --arg hook_id "$HOOK_ID" \
    --arg hook_type "$HOOK_TYPE" \
    --arg session_id "$SESSION_ID" \
    --arg pane_id "$PANE_ID" \
    --argjson data "$INPUT" \
    '{
        jsonrpc: "2.0",
        method: "source.ingest",
        id: 1,
        params: {
            source_kind: "claude_hooks",
            event: {
                hook_id: $hook_id,
                hook_type: $hook_type,
                session_id: $session_id,
                timestamp: (now | todate),
                pane_id: (if $pane_id == "" then null else $pane_id end),
                data: $data
            }
        }
    }')

# Send to daemon (fire-and-forget)
echo "$REQUEST" | socat - "UNIX-CONNECT:$SOCKET" > /dev/null 2>&1 || true
