#!/usr/bin/env bash
# contract/test-daemon-info.sh — daemon.info JSON-RPC endpoint contract test
#
# Verifies:
#   1. daemon.info returns a JSON-RPC response with nonce, version, pid fields
#   2. nonce is a non-empty string (format: <pid>-<nanos>)
#   3. version matches the running binary's --version output
#   4. pid is the actual daemon process PID
#   5. Response uses correct JSON-RPC 2.0 envelope (result field, id echoed)
#
# No real CLI needed: uses direct UDS JSON-RPC against the daemon.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SOCKET="/tmp/agtmux-e2e-daemoninfo-$$/agtmuxd.sock"

echo "=== test-daemon-info.sh ==="

daemon_start "$SOCKET" 500
sleep 1

# ── Helper: send JSON-RPC and capture response ───────────────────────────
# _uds_rpc SOCKET PAYLOAD → stdout: JSON response line
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

# ── Scenario 1: daemon.info returns valid JSON-RPC response ──────────────

REQUEST=$(jq -nc '{
    jsonrpc: "2.0",
    method:  "daemon.info",
    id:      42,
    params:  {}
}')

RESPONSE=$(_uds_rpc "$SOCKET" "$REQUEST")
log "daemon.info response: $RESPONSE"

if ! echo "$RESPONSE" | jq -e 'type == "object"' >/dev/null 2>&1; then
    fail "daemon.info response is not a JSON object"
fi
pass "Scenario 1: daemon.info returns JSON object"

# ── Scenario 2: JSON-RPC envelope is correct ─────────────────────────────

JSONRPC_VER=$(echo "$RESPONSE" | jq -r '.jsonrpc')
assert_eq "JSON-RPC version" "2.0" "$JSONRPC_VER"

RESPONSE_ID=$(echo "$RESPONSE" | jq -r '.id')
assert_eq "JSON-RPC id echoed" "42" "$RESPONSE_ID"

if ! echo "$RESPONSE" | jq -e '.result | type == "object"' >/dev/null 2>&1; then
    fail "daemon.info response missing .result object"
fi
pass "Scenario 2: JSON-RPC 2.0 envelope is correct"

# ── Scenario 3: nonce field is non-empty string ──────────────────────────

NONCE=$(echo "$RESPONSE" | jq -r '.result.nonce')
if [ -z "$NONCE" ] || [ "$NONCE" = "null" ]; then
    fail "Scenario 3: nonce is empty or null"
fi
# Nonce format: <pid>-<nanos> — must contain a hyphen
if ! echo "$NONCE" | grep -q '-'; then
    fail "Scenario 3: nonce format expected '<pid>-<nanos>', got '$NONCE'"
fi
pass "Scenario 3: nonce='$NONCE' (non-empty, contains hyphen)"

# ── Scenario 4: version matches binary --version ─────────────────────────

REPORTED_VERSION=$(echo "$RESPONSE" | jq -r '.result.version')
if [ -z "$REPORTED_VERSION" ] || [ "$REPORTED_VERSION" = "null" ]; then
    fail "Scenario 4: version is empty or null"
fi
# Version must be a semver-like string (at least X.Y.Z)
if ! echo "$REPORTED_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+'; then
    fail "Scenario 4: version '$REPORTED_VERSION' does not look like semver"
fi

pass "Scenario 4: version='$REPORTED_VERSION' (valid semver)"

# ── Scenario 5: pid matches actual daemon PID ────────────────────────────

REPORTED_PID=$(echo "$RESPONSE" | jq -r '.result.pid')
if [ -z "$REPORTED_PID" ] || [ "$REPORTED_PID" = "null" ]; then
    fail "Scenario 5: pid is empty or null"
fi
# DAEMON_PID is set by daemon_start in daemon.sh
assert_eq "pid matches daemon PID" "$DAEMON_PID" "$REPORTED_PID"

pass "Scenario 5: pid=$REPORTED_PID"

echo "=== test-daemon-info.sh PASS ==="
