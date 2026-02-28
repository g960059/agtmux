#!/usr/bin/env bash
# harness/daemon.sh — daemon lifecycle helpers for e2e tests
#
# Source this file after common.sh.
# Provides: daemon_start, daemon_stop
#
# Globals written:
#   DAEMON_PID — PID of the started daemon
#   E2E_SOCKET_DIR — temp directory holding the socket (cleaned up by daemon_stop)

DAEMON_PID=""
E2E_SOCKET_DIR=""

# daemon_start SOCKET [POLL_INTERVAL_MS]
# Starts the agtmux daemon in the background with the given socket path.
# Waits until the socket is ready.
daemon_start() {
    local socket="$1"
    local poll_ms="${2:-500}"

    E2E_SOCKET_DIR="$(dirname "$socket")"
    mkdir -p "$E2E_SOCKET_DIR"

    log "daemon_start: socket=$socket poll_ms=$poll_ms"

    # Build first if AGTMUX_BIN points at a cargo target (allow override for pre-built)
    if [ "${AGTMUX_SKIP_BUILD:-0}" = "0" ] && [ "${AGTMUX_BIN:-agtmux}" = "agtmux" ]; then
        # Use pre-built release binary if available; otherwise fall back to cargo run
        :
    fi

    if command -v "${AGTMUX_BIN:-agtmux}" >/dev/null 2>&1 && [ "${AGTMUX_BIN:-}" != "" ]; then
        "$AGTMUX_BIN" --socket-path "$socket" daemon --poll-interval-ms "$poll_ms" \
            >/tmp/agtmux-e2e-daemon-$$.log 2>&1 &
    else
        cargo run -p agtmux-runtime --quiet -- \
            --socket-path "$socket" daemon --poll-interval-ms "$poll_ms" \
            >/tmp/agtmux-e2e-daemon-$$.log 2>&1 &
    fi
    DAEMON_PID=$!
    log "daemon_start: PID=$DAEMON_PID"

    wait_for_socket "$socket" 15
    log "daemon_start: socket ready"
}

# daemon_stop
# Kills the daemon and cleans up.
daemon_stop() {
    if [ -n "$DAEMON_PID" ]; then
        log "daemon_stop: killing PID=$DAEMON_PID"
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
        DAEMON_PID=""
    fi
    if [ -n "$E2E_SOCKET_DIR" ]; then
        rm -rf "$E2E_SOCKET_DIR"
        E2E_SOCKET_DIR=""
    fi
}

# register_cleanup
# Sets up an EXIT trap to always call daemon_stop.
# Call once at the top of each test script.
register_cleanup() {
    trap daemon_stop EXIT
}
