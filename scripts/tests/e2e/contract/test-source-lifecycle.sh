#!/usr/bin/env bash
# contract/test-source-lifecycle.sh — source.hello + source.heartbeat + list_source_registry
#
# Verifies the source connection lifecycle at the contract level:
#   1. source.hello with valid protocol_version → accepted
#   2. source.hello with invalid protocol_version → rejected
#   3. list_source_registry returns the registered source entry
#   4. source.heartbeat for a registered source → acknowledged=true
#   5. source.heartbeat for unknown source → acknowledged=false
#   6. source.hello re-registration updates heartbeat
#   7. list_source_registry entry has correct fields (source_id, source_kind, lifecycle, etc.)
#
# These are the gateway handshake endpoints (server.rs lines 151-210).
# They have thorough unit tests in source_registry.rs but ZERO e2e coverage.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SOCKET="/tmp/agtmux-e2e-srclife-$$/agtmuxd.sock"

echo "=== test-source-lifecycle.sh ==="

daemon_start "$SOCKET" 500
sleep 1

# ── Helper: send JSON-RPC and capture response ───────────────────────────
_uds_rpc() {
    local socket="$1" payload="$2"
    python3 - "$socket" "$payload" <<'PYEOF'
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

# ── Scenario 1: source.hello accepted with valid protocol ────────────────

HELLO_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.hello",
    id:      1,
    params: {
        source_id:        "e2e-test-src-1",
        source_kind:      "claude_hooks",
        protocol_version: 1,
        socket_path:      "/tmp/e2e-test.sock"
    }
}')

HELLO_RESP=$(_uds_rpc "$SOCKET" "$HELLO_REQ")
log "source.hello response: $HELLO_RESP"

STATUS=$(echo "$HELLO_RESP" | jq -r '.result.status')
assert_eq "Scenario 1: source.hello status" "accepted" "$STATUS"

SOURCE_ID=$(echo "$HELLO_RESP" | jq -r '.result.source_id')
assert_eq "Scenario 1: source_id echoed" "e2e-test-src-1" "$SOURCE_ID"

pass "Scenario 1: source.hello accepted with protocol_version=1"

# ── Scenario 2: source.hello rejected with invalid protocol ──────────────

BAD_HELLO=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.hello",
    id:      2,
    params: {
        source_id:        "e2e-bad-proto",
        source_kind:      "claude_hooks",
        protocol_version: 999
    }
}')

BAD_RESP=$(_uds_rpc "$SOCKET" "$BAD_HELLO")
log "source.hello (bad proto) response: $BAD_RESP"

BAD_STATUS=$(echo "$BAD_RESP" | jq -r '.result.status')
assert_eq "Scenario 2: bad protocol status" "rejected" "$BAD_STATUS"

BAD_REASON=$(echo "$BAD_RESP" | jq -r '.result.reason')
assert_contains "Scenario 2: reason contains 'protocol'" "protocol" "$BAD_REASON"

pass "Scenario 2: source.hello rejected with protocol_version=999"

# ── Scenario 3: list_source_registry shows the registered source ─────────

LIST_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "list_source_registry",
    id:      3,
    params:  {}
}')

LIST_RESP=$(_uds_rpc "$SOCKET" "$LIST_REQ")
log "list_source_registry response: $LIST_RESP"

# Result should be an array
ENTRY_COUNT=$(echo "$LIST_RESP" | jq '.result | length')
if [ "$ENTRY_COUNT" -lt 1 ]; then
    fail "Scenario 3: list_source_registry returned 0 entries"
fi

# Find our registered source
FOUND_SRC=$(echo "$LIST_RESP" | jq -r '.result[] | select(.source_id=="e2e-test-src-1") | .source_id')
assert_eq "Scenario 3: e2e-test-src-1 in registry" "e2e-test-src-1" "$FOUND_SRC"

# The bad-proto source should NOT be registered (rejected)
BAD_SRC=$(echo "$LIST_RESP" | jq -r '.result[] | select(.source_id=="e2e-bad-proto") | .source_id')
if [ "$BAD_SRC" = "e2e-bad-proto" ]; then
    fail "Scenario 3: rejected source should not appear in registry"
fi
pass "Scenario 3: list_source_registry shows accepted source, excludes rejected"

# ── Scenario 4: list_source_registry entry has correct fields ────────────

ENTRY=$(echo "$LIST_RESP" | jq '.result[] | select(.source_id=="e2e-test-src-1")')
log "registry entry: $ENTRY"

ENTRY_KIND=$(echo "$ENTRY" | jq -r '.source_kind')
assert_eq "Scenario 4: source_kind" "claude_hooks" "$ENTRY_KIND"

ENTRY_LIFECYCLE=$(echo "$ENTRY" | jq -r '.lifecycle')
assert_eq "Scenario 4: lifecycle" "active" "$ENTRY_LIFECYCLE"

ENTRY_PROTO=$(echo "$ENTRY" | jq -r '.protocol_version')
assert_eq "Scenario 4: protocol_version" "1" "$ENTRY_PROTO"

ENTRY_SOCKET=$(echo "$ENTRY" | jq -r '.socket_path')
assert_eq "Scenario 4: socket_path" "/tmp/e2e-test.sock" "$ENTRY_SOCKET"

# registered_at_ms and last_heartbeat_ms must be positive numbers
REG_AT=$(echo "$ENTRY" | jq '.registered_at_ms')
if [ "$REG_AT" = "null" ] || [ "$REG_AT" = "0" ]; then
    fail "Scenario 4: registered_at_ms is null or zero"
fi

HB_AT=$(echo "$ENTRY" | jq '.last_heartbeat_ms')
if [ "$HB_AT" = "null" ] || [ "$HB_AT" = "0" ]; then
    fail "Scenario 4: last_heartbeat_ms is null or zero"
fi

pass "Scenario 4: registry entry has all required fields with correct values"

# ── Scenario 5: source.heartbeat for registered source → acknowledged ────

HB_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.heartbeat",
    id:      5,
    params: {
        source_id: "e2e-test-src-1"
    }
}')

HB_RESP=$(_uds_rpc "$SOCKET" "$HB_REQ")
log "source.heartbeat response: $HB_RESP"

ACKNOWLEDGED=$(echo "$HB_RESP" | jq -r '.result.acknowledged')
assert_eq "Scenario 5: heartbeat acknowledged" "true" "$ACKNOWLEDGED"

pass "Scenario 5: source.heartbeat acknowledged for registered source"

# ── Scenario 6: source.heartbeat for unknown source → not acknowledged ───

UNKNOWN_HB=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.heartbeat",
    id:      6,
    params: {
        source_id: "nonexistent-source-xyz"
    }
}')

UNKNOWN_RESP=$(_uds_rpc "$SOCKET" "$UNKNOWN_HB")
log "source.heartbeat (unknown) response: $UNKNOWN_RESP"

UNKNOWN_ACK=$(echo "$UNKNOWN_RESP" | jq -r '.result.acknowledged')
assert_eq "Scenario 6: unknown heartbeat not acknowledged" "false" "$UNKNOWN_ACK"

pass "Scenario 6: source.heartbeat returns acknowledged=false for unknown source"

# ── Scenario 7: source.hello with unknown source_kind → rejected ─────────

UNKNOWN_KIND=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.hello",
    id:      7,
    params: {
        source_id:        "e2e-unknown-kind",
        source_kind:      "unknown_provider",
        protocol_version: 1
    }
}')

UNKNOWN_KIND_RESP=$(_uds_rpc "$SOCKET" "$UNKNOWN_KIND")
log "source.hello (unknown kind) response: $UNKNOWN_KIND_RESP"

# Should be a JSON-RPC error (code -32602)
ERROR_CODE=$(echo "$UNKNOWN_KIND_RESP" | jq '.error.code')
assert_eq "Scenario 7: unknown source_kind error code" "-32602" "$ERROR_CODE"

pass "Scenario 7: source.hello rejects unknown source_kind with -32602"

# ── Scenario 8: re-hello updates socket_path ─────────────────────────────

REHELLO_REQ=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "source.hello",
    id:      8,
    params: {
        source_id:        "e2e-test-src-1",
        source_kind:      "claude_hooks",
        protocol_version: 1,
        socket_path:      "/tmp/e2e-test-updated.sock"
    }
}')

REHELLO_RESP=$(_uds_rpc "$SOCKET" "$REHELLO_REQ")
log "re-hello response: $REHELLO_RESP"

REHELLO_STATUS=$(echo "$REHELLO_RESP" | jq -r '.result.status')
assert_eq "Scenario 8: re-hello status" "accepted" "$REHELLO_STATUS"

# Verify socket_path updated in registry
LIST_RESP2=$(_uds_rpc "$SOCKET" "$LIST_REQ")
UPDATED_SOCKET=$(echo "$LIST_RESP2" | jq -r '.result[] | select(.source_id=="e2e-test-src-1") | .socket_path')
assert_eq "Scenario 8: socket_path updated" "/tmp/e2e-test-updated.sock" "$UPDATED_SOCKET"

pass "Scenario 8: re-hello updates socket_path in registry"

echo "=== test-source-lifecycle.sh PASS ==="
