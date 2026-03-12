#!/usr/bin/env bash
# contract/test-long-poll.sh — ui.wait_for_changes.v1 contract test

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SOCKET="/tmp/agtmux-e2e-longpoll-$$/agtmuxd.sock"

echo "=== test-long-poll.sh ==="

daemon_start "$SOCKET" 500

BOOTSTRAP="$(daemon_rpc "$SOCKET" "ui.bootstrap.v3")"
if ! echo "$BOOTSTRAP" | jq -e 'type == "object"' >/dev/null 2>&1; then
    fail "ui.bootstrap.v3 response is not a JSON object"
fi

CURSOR="$(echo "$BOOTSTRAP" | jq -c '.replay_cursor')"
if [ -z "$CURSOR" ] || [ "$CURSOR" = "null" ]; then
    fail "ui.bootstrap.v3 did not return replay_cursor"
fi

PARAMS="$(jq -nc --argjson cursor "$CURSOR" '{cursor: $cursor, timeout_ms: 500}')"
RESPONSE="$(daemon_rpc "$SOCKET" "ui.wait_for_changes.v1" "$PARAMS")"
log "ui.wait_for_changes.v1 response: $RESPONSE"

if ! echo "$RESPONSE" | jq -e 'type == "object"' >/dev/null 2>&1; then
    fail "ui.wait_for_changes.v1 response is not a JSON object"
fi

if ! echo "$RESPONSE" | jq -e 'has("changes") or has("resync_required")' >/dev/null 2>&1; then
    fail "ui.wait_for_changes.v1 response missing changes/resync_required"
fi

pass "ui.wait_for_changes.v1 returns valid JSON payload"

daemon_stop

echo "=== test-long-poll.sh PASS ==="
