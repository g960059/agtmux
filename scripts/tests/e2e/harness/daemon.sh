#!/usr/bin/env bash
# harness/daemon.sh — daemon lifecycle helpers for e2e tests
#
# Source this file after common.sh.
# Provides: daemon_start, daemon_stop
#
# Globals written:
#   DAEMON_PID — PID of the started daemon
#   E2E_SOCKET_DIR — temp directory holding the socket (cleaned up by daemon_stop)
#   AGTMUX_BIN — updated to the resolved binary path so client helpers match the daemon

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

    # Resolve daemon binary: AGTMUX_BIN must be an absolute path to use a pre-built binary.
    # If AGTMUX_BIN is unset, "agtmux", or a relative name, build from source via cargo.
    # This prevents accidentally using a globally-installed (potentially stale) agtmux.
    # After resolution, AGTMUX_BIN is updated so client helpers (jq_get, wait_for_agtmux_state)
    # use the same binary version as the daemon.
    local _daemon_bin="${AGTMUX_BIN:-}"
    if [ -n "$_daemon_bin" ] && [[ "$_daemon_bin" == /* ]] && command -v "$_daemon_bin" >/dev/null 2>&1; then
        "$_daemon_bin" --socket-path "$socket" daemon --poll-interval-ms "$poll_ms" \
            >/tmp/agtmux-e2e-daemon-$$.log 2>&1 &
        AGTMUX_BIN="$_daemon_bin"
    else
        # Build the latest binary, then run it directly (avoids cargo startup overhead).
        if [ "${AGTMUX_SKIP_BUILD:-0}" = "0" ]; then
            cargo build -p agtmux --quiet 2>/dev/null
        fi
        # Find the cargo-built binary relative to the repo root.
        local _repo_root
        _repo_root="$(git rev-parse --show-toplevel 2>/dev/null || echo ".")"
        local _built_bin="${_repo_root}/target/debug/agtmux"
        if [ -x "$_built_bin" ]; then
            "$_built_bin" --socket-path "$socket" daemon --poll-interval-ms "$poll_ms" \
                >/tmp/agtmux-e2e-daemon-$$.log 2>&1 &
            AGTMUX_BIN="$_built_bin"
        else
            cargo run -p agtmux --quiet -- \
                --socket-path "$socket" daemon --poll-interval-ms "$poll_ms" \
                >/tmp/agtmux-e2e-daemon-$$.log 2>&1 &
        fi
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
