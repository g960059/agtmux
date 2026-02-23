#!/usr/bin/env bash
# Shared functions for AGTMUX live tests.

# ── Logging ──────────────────────────────────────────────────────────────────

log_info()  { printf '[INFO]  %s\n' "$*"; }
log_error() { printf '[ERROR] %s\n' "$*" >&2; }
log_pass()  { printf '[PASS]  %s\n' "$*"; }
log_fail()  { printf '[FAIL]  %s\n' "$*" >&2; }

# ── Cleanup ──────────────────────────────────────────────────────────────────

# Globals expected to be set by the caller before registering cleanup:
#   DAEMON_PID, TMUX_SESSION, TMPDIR_LIVE
cleanup() {
    log_info "cleanup: starting"
    if [ -n "${DAEMON_PID:-}" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    if [ -n "${TMUX_SESSION:-}" ]; then
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
    fi
    if [ -n "${TMPDIR_LIVE:-}" ] && [ -d "$TMPDIR_LIVE" ]; then
        rm -rf "$TMPDIR_LIVE"
    fi
    log_info "cleanup: done"
}

# ── Utilities ────────────────────────────────────────────────────────────────

# wait_for_file FILE TIMEOUT_SEC
#   Poll until FILE exists (or TIMEOUT_SEC expires). Returns 1 on timeout.
wait_for_file() {
    local file="$1"
    local timeout="$2"
    local elapsed=0
    while [ ! -e "$file" ]; do
        sleep 1
        elapsed=$((elapsed + 1))
        if [ "$elapsed" -ge "$timeout" ]; then
            return 1
        fi
    done
    return 0
}
