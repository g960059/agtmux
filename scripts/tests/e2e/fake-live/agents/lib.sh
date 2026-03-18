#!/usr/bin/env bash
# fake-live/agents/lib.sh — shared helpers for fake provider drivers

set -euo pipefail

write_tty_line() {
    local tty="$1" line="$2"
    printf '%s\r\n' "$line" >"$tty"
}

sleep_with_jitter() {
    local seconds="${1:-1}"
    sleep "$seconds"
}

kill_runtime_pid() {
    local runtime_pid="$1"
    kill "$runtime_pid" 2>/dev/null || true
    local _=0
    while kill -0 "$runtime_pid" 2>/dev/null; do
        sleep 0.2
        _=$((_ + 1))
        [ "$_" -lt 25 ] || break
    done
}

send_uds_payload() {
    local socket="$1" payload="$2"
    if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
        local py
        py="$(command -v python3 2>/dev/null || command -v python)"
        "$py" - "$socket" "$payload" <<'PYEOF'
import socket
import sys

sock_path = sys.argv[1]
payload = sys.argv[2]

with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
    client.connect(sock_path)
    client.sendall(payload.encode() + b"\n")
    try:
        client.recv(4096)
    except Exception:
        pass
PYEOF
        return 0
    fi

    if command -v socat >/dev/null 2>&1; then
        printf '%s\n' "$payload" | socat -t 1 - "UNIX-CONNECT:$socket" >/dev/null
        return 0
    fi

    echo "[fake-agent] missing socat/python for UDS send" >&2
    return 1
}

send_claude_hook() {
    local socket="$1" pane_id="$2" hook_type="$3" session_id="$4"
    local notification_type="${5:-}"
    local tool_name="${6:-}"
    local ts="" hook_id="" payload=""

    ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    hook_id="fake-${hook_type//[^[:alnum:]]/-}-$(date +%s)-$$"

    payload="$(jq -nc \
        --arg hook_type "$hook_type" \
        --arg session_id "$session_id" \
        --arg pane_id "$pane_id" \
        --arg hook_id "$hook_id" \
        --arg timestamp "$ts" \
        --arg notification_type "$notification_type" \
        --arg tool_name "$tool_name" \
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
                    timestamp: $timestamp,
                    pane_id: $pane_id,
                    data: (
                        {}
                        + (if $notification_type == "" then {} else {notification_type: $notification_type} end)
                        + (if $tool_name == "" then {} else {tool: $tool_name} end)
                    )
                }
            }
        }')"

    send_uds_payload "$socket" "$payload"
}
