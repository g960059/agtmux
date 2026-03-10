#!/usr/bin/env bash
# harness/common.sh — shared utilities for agtmux contract e2e tests
#
# Source this file at the top of each test script:
#   source "$(dirname "$0")/../harness/common.sh"
#
# Dependencies: agtmux (in PATH or AGTMUX_BIN), jq

set -euo pipefail

# ── Binary resolution ──────────────────────────────────────────────────────

# Allow tests to override the binary path. Otherwise prefer the freshly built local binary.
if [ -n "${AGTMUX_BIN:-}" ]; then
    AGTMUX_BIN="${AGTMUX_BIN}"
elif [ -x "$(pwd)/target/debug/agtmux" ]; then
    AGTMUX_BIN="$(pwd)/target/debug/agtmux"
elif [ -x "$(pwd)/target/release/agtmux" ]; then
    AGTMUX_BIN="$(pwd)/target/release/agtmux"
else
    AGTMUX_BIN="agtmux"
fi

# ── Logging / assertion helpers ────────────────────────────────────────────

log() {
    echo "[$(date +%H:%M:%S)] $*" >&2
}

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

pass() {
    echo "[PASS] $*" >&2
}

assert_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "$actual" = "$expected" ]; then
        pass "$label: got '$actual'"
    else
        fail "$label: expected '$expected' got '$actual'"
    fi
}

assert_contains() {
    local label="$1" needle="$2" haystack="$3"
    if echo "$haystack" | grep -qF "$needle"; then
        pass "$label: found '$needle'"
    else
        fail "$label: '$needle' not found in output"
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" haystack="$3"
    if echo "$haystack" | grep -qF "$needle"; then
        fail "$label: '$needle' should NOT be in output"
    else
        pass "$label: '$needle' correctly absent"
    fi
}

# ── agtmux JSON field getter ───────────────────────────────────────────────

# jq_get SOCKET PANE_ID FIELD
# Returns the raw value of .FIELD for the pane with matching pane_id.
# Returns "null" if the pane is not found.
jq_get() {
    local socket="$1" pane_id="$2" field="$3"
    "$AGTMUX_BIN" --socket-path "$socket" json 2>/dev/null \
        | jq -r --arg p "$pane_id" --arg f "$field" \
            '.panes[] | select(.pane_id==$p) | .[$f] // "null"' \
        2>/dev/null || echo "null"
}

# ── Daemon JSON-RPC helpers ───────────────────────────────────────────────

daemon_rpc() {
    local socket="$1" method="$2"
    local params="${3:-{}}"

    if command -v python3 >/dev/null 2>&1; then
        python3 - "$socket" "$method" "$params" <<'PY'
import json
import socket
import sys

socket_path, method, params_json = sys.argv[1], sys.argv[2], sys.argv[3]
params = json.loads(params_json)
request = {
    "jsonrpc": "2.0",
    "method": method,
    "params": params,
    "id": 1,
}

with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode("utf-8"))
    client.shutdown(socket.SHUT_WR)
    response = b""
    while True:
        chunk = client.recv(65536)
        if not chunk:
            break
        response += chunk

payload = json.loads(response.decode("utf-8").strip())
if "error" in payload:
    raise SystemExit(json.dumps(payload["error"]))
print(json.dumps(payload.get("result")))
PY
        return
    fi

    if command -v python >/dev/null 2>&1; then
        python - "$socket" "$method" "$params" <<'PY'
import json
import socket
import sys

socket_path, method, params_json = sys.argv[1], sys.argv[2], sys.argv[3]
params = json.loads(params_json)
request = {
    "jsonrpc": "2.0",
    "method": method,
    "params": params,
    "id": 1,
}

client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect(socket_path)
client.sendall((json.dumps(request) + "\n").encode("utf-8"))
client.shutdown(socket.SHUT_WR)
response = b""
while True:
    chunk = client.recv(65536)
    if not chunk:
        break
    response += chunk
client.close()

payload = json.loads(response.decode("utf-8").strip())
if "error" in payload:
    raise SystemExit(json.dumps(payload["error"]))
print(json.dumps(payload.get("result")))
PY
        return
    fi

    fail "python3 or python is required for daemon_rpc"
}

# ── State polling ──────────────────────────────────────────────────────────

# wait_for_agtmux_state SOCKET PANE_ID FIELD EXPECTED [MAX_WAIT_S]
# Polls list-panes --json every second until FIELD == EXPECTED or timeout.
# Exits non-zero on timeout.
wait_for_agtmux_state() {
    local socket="$1" pane_id="$2" field="$3" expected="$4"
    local max_wait="${5:-60}"
    local elapsed=0 actual=""

    while [ "$elapsed" -lt "$max_wait" ]; do
        actual=$(jq_get "$socket" "$pane_id" "$field")
        if [ "$actual" = "$expected" ]; then
            log "wait_for_agtmux_state OK: pane=$pane_id $field='$expected' (${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done

    echo "[FAIL] timeout(${max_wait}s): pane=$pane_id field=$field expected='$expected' actual='$actual'" >&2
    "$AGTMUX_BIN" --socket-path "$socket" json 2>/dev/null | jq '.panes' >&2 || true
    return 1
}

# wait_for_agtmux_state_any SOCKET PANE_ID FIELD "EXPECTED1 EXPECTED2 ..." [MAX_WAIT_S]
# Polls list-panes --json every second until FIELD matches any expected value.
wait_for_agtmux_state_any() {
    local socket="$1" pane_id="$2" field="$3" expected_values="$4"
    local max_wait="${5:-60}"
    local elapsed=0 actual="" expected=""

    while [ "$elapsed" -lt "$max_wait" ]; do
        actual=$(jq_get "$socket" "$pane_id" "$field")
        for expected in $expected_values; do
            if [ "$actual" = "$expected" ]; then
                log "wait_for_agtmux_state_any OK: pane=$pane_id $field='$actual' (${elapsed}s)"
                return 0
            fi
        done
        sleep 1
        elapsed=$((elapsed + 1))
    done

    echo "[FAIL] timeout(${max_wait}s): pane=$pane_id field=$field expected_any='$expected_values' actual='$actual'" >&2
    "$AGTMUX_BIN" --socket-path "$socket" json 2>/dev/null | jq '.panes' >&2 || true
    return 1
}

is_shell_cmd() {
    local cmd
    cmd="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
    case "$cmd" in
        zsh|bash|fish|sh|csh|tcsh|ksh|dash|nu|pwsh) return 0 ;;
        *) return 1 ;;
    esac
}

# wait_for_completion_or_shell_demotion SOCKET PANE_ID [MAX_WAIT_S]
# Accepts either:
#   - managed completion (`activity_state` reached `waiting_input` / `idle`)
#   - exact-row shell demotion (`presence=unmanaged`, `provider=null`,
#     `activity_state=null`, `current_cmd` is a plain shell)
# Prints `managed` or `unmanaged` to stdout for callers that need to branch.
wait_for_completion_or_shell_demotion() {
    local socket="$1" pane_id="$2"
    local max_wait="${3:-60}"
    local elapsed=0 presence="" provider="" activity="" current_cmd=""

    while [ "$elapsed" -lt "$max_wait" ]; do
        activity=$(jq_get "$socket" "$pane_id" "activity_state")
        presence=$(jq_get "$socket" "$pane_id" "presence")
        if [ "$presence" = "managed" ] && { [ "$activity" = "waiting_input" ] || [ "$activity" = "idle" ]; }; then
            log "wait_for_completion_or_shell_demotion OK: pane=$pane_id completed as managed (${elapsed}s)"
            printf 'managed\n'
            return 0
        fi

        provider=$(jq_get "$socket" "$pane_id" "provider")
        current_cmd=$(jq_get "$socket" "$pane_id" "current_cmd")
        if [ "$presence" = "unmanaged" ] \
            && [ "$provider" = "null" ] \
            && [ "$activity" = "null" ] \
            && is_shell_cmd "$current_cmd"; then
            log "wait_for_completion_or_shell_demotion OK: pane=$pane_id demoted to shell truth (${elapsed}s)"
            printf 'unmanaged\n'
            return 0
        fi

        sleep 1
        elapsed=$((elapsed + 1))
    done

    echo "[FAIL] timeout(${max_wait}s): pane=$pane_id expected managed completion or shell demotion" >&2
    "$AGTMUX_BIN" --socket-path "$socket" json 2>/dev/null | jq '.panes' >&2 || true
    return 1
}

# wait_for_socket SOCKET [MAX_WAIT_S]
# Waits until the UDS socket file exists (daemon ready).
wait_for_socket() {
    local socket="$1" max_wait="${2:-15}"
    local elapsed_half=0  # counts in 0.5s increments; max_wait*2 = total half-steps
    local max_half=$((max_wait * 2))
    while [ "$elapsed_half" -lt "$max_half" ]; do
        [ -S "$socket" ] && return 0
        sleep 0.5
        elapsed_half=$((elapsed_half + 1))
    done
    fail "daemon socket not ready after ${max_wait}s: $socket"
}
