#!/usr/bin/env bash
# harness/inject.sh — synthetic event injection for contract e2e tests
#
# Source this file after common.sh.
# Uses socat (preferred) or nc -U (macOS fallback) to send JSON-RPC
# source.ingest events to the daemon UDS.
#
# Provides:
#   inject_claude_event SOCKET HOOK_TYPE SESSION_ID PANE_ID
#   inject_codex_event  SOCKET EVENT_TYPE THREAD_ID PANE_ID [CWD]

# _uds_send SOCKET PAYLOAD — send newline-terminated payload to UDS
# Priority: socat > python3 > python
# macOS nc -U has a race condition with stdin pipes; avoid it.
_uds_send() {
    local socket="$1" payload="$2"
    if command -v socat >/dev/null 2>&1; then
        printf '%s\n' "$payload" | socat - "UNIX-CONNECT:$socket" >/dev/null 2>&1 || true
    elif command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
        local py; py=$(command -v python3 2>/dev/null || command -v python)
        "$py" - "$socket" "$payload" <<'PYEOF' 2>/dev/null || true
import socket as _s, sys

sock_path = sys.argv[1]
payload   = sys.argv[2]

with _s.socket(_s.AF_UNIX, _s.SOCK_STREAM) as s:
    s.connect(sock_path)
    s.sendall(payload.encode() + b"\n")
    try:
        s.recv(4096)   # consume response so server can flush
    except Exception:
        pass
PYEOF
    else
        log "WARN: socat and python not found; cannot inject events via UDS"
    fi
}

# inject_claude_event SOCKET HOOK_TYPE SESSION_ID PANE_ID
# Injects a claude_hooks source.ingest event.
# hook_type: tool_start | tool_end | idle | stop | notification | wait_for_approval
inject_claude_event() {
    local socket="$1" hook_type="$2" session_id="$3" pane_id="$4"
    local ts; ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local hook_id="h-e2e-$$-$(date +%s%N)"

    local payload
    payload=$(jq -nc \
        --arg ht  "$hook_type"  \
        --arg sid "$session_id" \
        --arg pid "$pane_id"    \
        --arg hid "$hook_id"    \
        --arg ts  "$ts"         \
        '{
            jsonrpc: "2.0",
            method:  "source.ingest",
            id:      1,
            params: {
                source_kind: "claude_hooks",
                event: {
                    hook_id:    $hid,
                    hook_type:  $ht,
                    session_id: $sid,
                    timestamp:  $ts,
                    pane_id:    (if $pid == "" then null else $pid end),
                    data:       {}
                }
            }
        }')

    log "inject_claude_event: hook_type=$hook_type pane_id=$pane_id"
    _uds_send "$socket" "$payload"
}

# inject_claude_event_loop SOCKET HOOK_TYPE SESSION_ID PANE_ID [INTERVAL_S]
# Injects claude events continuously in the background.
# The resolver requires events < 3s old; continuous injection ensures freshness.
# Prints the background PID so callers can kill it later.
inject_claude_event_loop() {
    local socket="$1" hook_type="$2" session_id="$3" pane_id="$4"
    local interval="${5:-1}"
    # Redirect stdout to /dev/null so the background subshell does not hold the
    # $() pipe open when the caller does: INJECTOR_PID=$(inject_claude_event_loop ...)
    # Without this redirect the $() waits forever for the infinite loop to exit.
    (
        while true; do
            inject_claude_event "$socket" "$hook_type" "$session_id" "$pane_id"
            sleep "$interval"
        done
    ) >/dev/null &
    echo $!
}

# inject_codex_event SOCKET EVENT_TYPE THREAD_ID PANE_ID [CWD]
# Injects a codex_appserver source.ingest event.
# event_type mirrors codex_poller internal names:
#   thread.active | thread.idle | thread.not_loaded | thread.error
#   turn.started  | turn.completed | turn.interrupted | turn.failed
inject_codex_event() {
    local socket="$1" event_type="$2" thread_id="$3" pane_id="$4"
    local cwd="${5:-/tmp}"
    local ts; ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)

    local payload
    payload=$(jq -nc \
        --arg et  "$event_type"  \
        --arg tid "$thread_id"  \
        --arg pid "$pane_id"    \
        --arg cwd "$cwd"        \
        --arg ts  "$ts"         \
        '{
            jsonrpc: "2.0",
            method:  "source.ingest",
            id:      2,
            params: {
                source_kind: "codex_appserver",
                event: {
                    id:         $tid,
                    event_type: $et,
                    session_id: $tid,
                    pane_id:    (if $pid == "" then null else $pid end),
                    cwd:        $cwd,
                    timestamp:  $ts,
                    payload:    {},
                    is_heartbeat: false
                }
            }
        }')

    log "inject_codex_event: event_type=$event_type pane_id=$pane_id"
    _uds_send "$socket" "$payload"
}

# inject_codex_event_loop SOCKET EVENT_TYPE THREAD_ID PANE_ID [CWD] [INTERVAL_S]
# Injects codex events continuously in the background.
# event_type: thread.active | thread.idle | thread.not_loaded | thread.error | turn.started | ...
inject_codex_event_loop() {
    local socket="$1" event_type="$2" thread_id="$3" pane_id="$4"
    local cwd="${5:-/tmp}" interval="${6:-1}"
    # Same /dev/null redirect as inject_claude_event_loop (see comment above).
    (
        while true; do
            inject_codex_event "$socket" "$event_type" "$thread_id" "$pane_id" "$cwd"
            sleep "$interval"
        done
    ) >/dev/null &
    echo $!
}
