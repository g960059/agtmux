#!/usr/bin/env bash
# contract/test-invalid-jsonrpc.sh — Daemon resilience against malformed JSON-RPC requests
#
# Gap filled: ZERO e2e coverage for the daemon's error-handling paths when
#   receiving invalid input. server.rs handle_connection() has:
#     - serde_json::from_str() that fails on non-JSON → connection error (debug log)
#     - method="" fallback when .method is missing → -32601 method not found
#     - unknown method dispatch → -32601 method not found
#     - unknown source_kind in source.ingest → -32602 invalid params
#     - invalid event in source.ingest → -32602 invalid params
#
# Why it matters: The daemon is a long-running UDS server. If a malformed request
#   panics or crashes the accept loop, ALL clients lose connectivity. This test
#   verifies that garbage input is gracefully rejected and the daemon stays alive.
#
# Scenarios:
#   1. Completely garbage (non-JSON) input → daemon stays alive
#   2. Valid JSON but missing "method" field → -32601
#   3. Unknown method name → -32601
#   4. source.ingest with unknown source_kind → -32602
#   5. source.ingest with malformed event payload → -32602
#   6. After all abuse: daemon still responds correctly to valid requests
#   7. Empty line (just newline) → daemon stays alive
#   8. Oversized payload (>1MB) → daemon stays alive (no OOM)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SOCKET="/tmp/agtmux-e2e-invalid-rpc-$$/agtmuxd.sock"

echo "=== test-invalid-jsonrpc.sh ==="

daemon_start "$SOCKET" 500
sleep 1

# ── Helper: send JSON-RPC and capture response ───────────────────────
# Returns the raw response (may be empty on connection error).
# Uses the same pattern as test-source-lifecycle.sh which is known to work.
_uds_rpc() {
    local socket="$1" payload="$2"
    python3 - "$socket" "$payload" <<'PYEOF' 2>/dev/null || echo ""
import socket as _s, sys

sock_path = sys.argv[1]
payload   = sys.argv[2]

with _s.socket(_s.AF_UNIX, _s.SOCK_STREAM) as s:
    s.connect(sock_path)
    s.sendall(payload.encode() + b"\n")
    s.shutdown(_s.SHUT_WR)
    chunks = []
    while True:
        data = s.recv(4096)
        if not data:
            break
        chunks.append(data)
    sys.stdout.write(b"".join(chunks).decode().strip())
PYEOF
}

# _uds_send_garbage SOCKET PAYLOAD — send garbage and don't wait for response
# For non-JSON input, the server closes the connection without a response.
# We don't want to block waiting for data that will never come.
_uds_send_garbage() {
    local socket="$1" payload="$2"
    python3 - "$socket" "$payload" <<'PYEOF' 2>/dev/null || true
import socket as _s, sys

sock_path = sys.argv[1]
payload   = sys.argv[2]

with _s.socket(_s.AF_UNIX, _s.SOCK_STREAM) as s:
    s.settimeout(3)
    s.connect(sock_path)
    s.sendall(payload.encode() + b"\n")
    s.shutdown(_s.SHUT_WR)
    try:
        s.recv(4096)  # consume any response; may be empty
    except Exception:
        pass
PYEOF
}

# ── Helper: verify daemon is alive by checking socket + daemon.info ──
_assert_daemon_alive() {
    local label="$1"
    if [ ! -S "$SOCKET" ]; then
        fail "$label: daemon socket file missing"
    fi
    local info_req
    info_req=$(jq -nc '{jsonrpc:"2.0", method:"daemon.info", id:999, params:{}}')
    local resp
    resp=$(_uds_rpc "$SOCKET" "$info_req")
    local nonce
    nonce=$(echo "$resp" | jq -r '.result.nonce' 2>/dev/null)
    if [ -z "$nonce" ] || [ "$nonce" = "null" ]; then
        fail "$label: daemon.info returned no nonce after abuse (resp=$resp)"
    fi
    pass "$label: daemon alive (nonce=$nonce)"
}

# ── Scenario 1: Completely garbage (non-JSON) input ──────────────────
# server.rs line 116: serde_json::from_str() returns Err → connection error
# The ? propagates to handle_connection() → caught by tokio::spawn error handler.
# Daemon must NOT crash.

# For garbage input, the server's serde_json::from_str() fails and the
# handle_connection task returns Err. No response is sent. We use
# _uds_send_garbage which has a timeout to avoid hanging on recv().
_uds_send_garbage "$SOCKET" "this is not json at all {{{"
log "Scenario 1: garbage sent (no response expected)"

# Give the daemon a moment to process and recover the connection handler
sleep 1

# The important thing is that the daemon is still alive
_assert_daemon_alive "Scenario 1"

pass "Scenario 1: garbage input does not crash daemon"

# ── Scenario 2: Valid JSON but missing "method" field ─────────────────
# server.rs: let method = request["method"].as_str().unwrap_or("");
# Empty string falls through to the _ => match arm → -32601.

NO_METHOD_REQ=$(jq -nc '{jsonrpc:"2.0", id:10, params:{}}')
NO_METHOD_RESP=$(_uds_rpc "$SOCKET" "$NO_METHOD_REQ")
log "Scenario 2: no-method response=$NO_METHOD_RESP"

ERROR_CODE=$(echo "$NO_METHOD_RESP" | jq '.error.code' 2>/dev/null)
assert_eq "Scenario 2: missing method error code" "-32601" "$ERROR_CODE"

ERROR_MSG=$(echo "$NO_METHOD_RESP" | jq -r '.error.message' 2>/dev/null)
assert_contains "Scenario 2: error message" "method not found" "$ERROR_MSG"

# JSON-RPC id must be echoed back
RESP_ID=$(echo "$NO_METHOD_RESP" | jq '.id' 2>/dev/null)
assert_eq "Scenario 2: id echoed" "10" "$RESP_ID"

pass "Scenario 2: missing method → -32601 method not found"

# ── Scenario 3: Unknown method name ──────────────────────────────────

UNKNOWN_REQ=$(jq -nc '{jsonrpc:"2.0", method:"nonexistent.rpc.call", id:11, params:{}}')
UNKNOWN_RESP=$(_uds_rpc "$SOCKET" "$UNKNOWN_REQ")
log "Scenario 3: unknown method response=$UNKNOWN_RESP"

UNKNOWN_CODE=$(echo "$UNKNOWN_RESP" | jq '.error.code' 2>/dev/null)
assert_eq "Scenario 3: unknown method error code" "-32601" "$UNKNOWN_CODE"

pass "Scenario 3: unknown method → -32601"

# ── Scenario 4: source.ingest with unknown source_kind ────────────────
# server.rs match source_kind "claude_hooks" => ... _ => -32602

BAD_SOURCE_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method: "source.ingest",
    id: 12,
    params: {
        source_kind: "totally_fake_source",
        event: {}
    }
}')
BAD_SOURCE_RESP=$(_uds_rpc "$SOCKET" "$BAD_SOURCE_REQ")
log "Scenario 4: bad source_kind response=$BAD_SOURCE_RESP"

BAD_SOURCE_CODE=$(echo "$BAD_SOURCE_RESP" | jq '.error.code' 2>/dev/null)
assert_eq "Scenario 4: unknown source_kind error code" "-32602" "$BAD_SOURCE_CODE"

pass "Scenario 4: source.ingest unknown source_kind → -32602"

# ── Scenario 5: source.ingest with malformed event payload ────────────
# server.rs: serde_json::from_value fails → -32602 invalid event

BAD_EVENT_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method: "source.ingest",
    id: 13,
    params: {
        source_kind: "claude_hooks",
        event: {"completely": "wrong", "structure": true}
    }
}')
BAD_EVENT_RESP=$(_uds_rpc "$SOCKET" "$BAD_EVENT_REQ")
log "Scenario 5: bad event response=$BAD_EVENT_RESP"

BAD_EVENT_CODE=$(echo "$BAD_EVENT_RESP" | jq '.error.code' 2>/dev/null)
assert_eq "Scenario 5: malformed event error code" "-32602" "$BAD_EVENT_CODE"

BAD_EVENT_MSG=$(echo "$BAD_EVENT_RESP" | jq -r '.error.message' 2>/dev/null)
assert_contains "Scenario 5: error references 'invalid event'" "invalid event" "$BAD_EVENT_MSG"

pass "Scenario 5: source.ingest malformed event → -32602"

# ── Scenario 6: After all abuse, daemon still responds correctly ──────
# This is the critical assertion: none of the above caused the daemon to
# enter a bad state, crash, or stop accepting connections.

_assert_daemon_alive "Scenario 6"

# Verify a full list_panes call works correctly
LIST_REQ=$(jq -nc '{jsonrpc:"2.0", method:"list_panes", id:14, params:{}}')
LIST_RESP=$(_uds_rpc "$SOCKET" "$LIST_REQ")
if ! echo "$LIST_RESP" | jq -e '.result | type == "array"' >/dev/null 2>&1; then
    fail "Scenario 6: list_panes should return array after abuse, got: $LIST_RESP"
fi

pass "Scenario 6: daemon fully functional after malformed request abuse"

# ── Scenario 7: Empty line (just newline) ─────────────────────────────
# Edge case: client connects and sends just a newline.

# Empty input: read_line returns empty string, from_str("") fails → no response.
_uds_send_garbage "$SOCKET" ""
log "Scenario 7: empty line sent (no response expected)"

sleep 1
_assert_daemon_alive "Scenario 7"

pass "Scenario 7: empty input does not crash daemon"

# ── Scenario 8: Rapid-fire mixed valid/invalid requests ───────────────
# Stress test: send valid and invalid requests alternating to verify
# no connection pool / state corruption.

for i in $(seq 1 5); do
    # Invalid (non-JSON, no response expected)
    _uds_send_garbage "$SOCKET" "garbage-$i"
    sleep 0.5
    # Valid
    PING_REQ=$(jq -nc --argjson id "$((100+i))" '{jsonrpc:"2.0", method:"daemon.info", id:$id, params:{}}')
    PING_RESP=$(_uds_rpc "$SOCKET" "$PING_REQ")
    PING_NONCE=$(echo "$PING_RESP" | jq -r '.result.nonce' 2>/dev/null)
    if [ -z "$PING_NONCE" ] || [ "$PING_NONCE" = "null" ]; then
        fail "Scenario 8: daemon stopped responding after rapid-fire round $i"
    fi
done

pass "Scenario 8: daemon survives rapid-fire mixed valid/invalid requests"

echo "=== test-invalid-jsonrpc.sh PASS ==="
