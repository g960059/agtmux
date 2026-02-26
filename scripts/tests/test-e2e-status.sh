#!/usr/bin/env bash
set -euo pipefail

# E2E smoke test: start daemon, wait for socket, run status, verify output.

SOCKET_DIR="/tmp/agtmux-e2e-test-$$"
SOCKET="$SOCKET_DIR/agtmuxd.sock"
DAEMON_PID=""

cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -f "$SOCKET"
    rmdir "$SOCKET_DIR" 2>/dev/null || true
}
trap cleanup EXIT

echo "[e2e] building agtmux..."
cargo build -p agtmux-runtime --quiet

echo "[e2e] starting daemon (socket=$SOCKET)..."
cargo run -p agtmux-runtime --quiet -- --socket-path "$SOCKET" daemon --poll-interval-ms 500 &
DAEMON_PID=$!

# Wait for socket to appear
READY=0
for _ in $(seq 1 10); do
    if [ -S "$SOCKET" ]; then
        READY=1
        break
    fi
    sleep 0.5
done

if [ "$READY" = "0" ]; then
    echo "[e2e] FAIL: socket not ready after 5s"
    exit 1
fi

echo "[e2e] socket ready, running status..."
# Give the daemon one poll tick to populate data
sleep 1.5

OUTPUT=$(cargo run -p agtmux-runtime --quiet -- --socket-path "$SOCKET" status 2>&1) || true
echo "$OUTPUT"

# Also test tmux-status
TMUX_STATUS=$(cargo run -p agtmux-runtime --quiet -- --socket-path "$SOCKET" tmux-status 2>&1) || true
echo "[e2e] tmux-status: $TMUX_STATUS"

# Verify output contains expected patterns
if echo "$OUTPUT" | grep -q "Panes:"; then
    echo "[e2e] PASS: status output OK"
else
    echo "[e2e] FAIL: unexpected status output"
    exit 1
fi

if echo "$TMUX_STATUS" | grep -qE "^A:[0-9]+ U:[0-9]+$"; then
    echo "[e2e] PASS: tmux-status output OK"
else
    echo "[e2e] FAIL: unexpected tmux-status output"
    exit 1
fi

echo "[e2e] ALL PASS"
