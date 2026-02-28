#!/usr/bin/env bash
# harness/common.sh — shared utilities for agtmux contract e2e tests
#
# Source this file at the top of each test script:
#   source "$(dirname "$0")/../harness/common.sh"
#
# Dependencies: agtmux (in PATH or AGTMUX_BIN), jq

set -euo pipefail

# ── Binary resolution ──────────────────────────────────────────────────────

# Allow tests to override the binary path (e.g. cargo run vs release build)
AGTMUX_BIN="${AGTMUX_BIN:-agtmux}"

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
