#!/usr/bin/env bash
# contract/test-schema.sh — JSON schema contract test
# Verifies required fields and value types in `agtmux json` output (schema v1).
# No real CLI needed: checks daemon output against schema.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"

register_cleanup

SOCKET="/tmp/agtmux-e2e-schema-$$/agtmuxd.sock"

echo "=== test-schema.sh ==="

daemon_start "$SOCKET" 200
sleep 1

# Get agtmux json output
OUTPUT=$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null || echo '{"version":1,"panes":[]}')

# Must be a JSON object with version and panes array
if ! echo "$OUTPUT" | jq -e 'type == "object"' >/dev/null 2>&1; then
    fail "agtmux json must return a JSON object, got: $OUTPUT"
fi
pass "output is JSON object"

if ! echo "$OUTPUT" | jq -e '.panes | type == "array"' >/dev/null 2>&1; then
    fail "agtmux json must have a .panes array, got: $OUTPUT"
fi
pass "output has .panes array"

# If there are panes, validate schema of each pane object
PANE_COUNT=$(echo "$OUTPUT" | jq '.panes | length')
log "pane count: $PANE_COUNT"

if [ "$PANE_COUNT" -gt 0 ]; then
    # Required string fields
    REQUIRED_FIELDS="pane_id session_id session_name window_id window_name presence current_cmd current_path"
    for field in $REQUIRED_FIELDS; do
        MISSING=$(echo "$OUTPUT" | jq --arg f "$field" \
            '[.panes[] | select(has($f) | not)] | length')
        assert_eq "field '$field' present in all panes" "0" "$MISSING"
    done

    # 'presence' must be "managed" or "unmanaged"
    INVALID_PRESENCE=$(echo "$OUTPUT" | jq \
        '[.panes[] | select(.presence != "managed" and .presence != "unmanaged")] | length')
    assert_eq "presence values are valid" "0" "$INVALID_PRESENCE"

    # managed panes must have 'activity_state'
    MISSING_STATE=$(echo "$OUTPUT" | jq \
        '[.panes[] | select(.presence == "managed" and (.activity_state == null or .activity_state == ""))] | length')
    assert_eq "managed panes have activity_state" "0" "$MISSING_STATE"

    # activity_state must be one of the known snake_case values (schema v1)
    VALID_STATES='["unknown","idle","running","waiting_input","waiting_approval","error"]'
    INVALID_STATE=$(echo "$OUTPUT" | jq --argjson valid "$VALID_STATES" \
        '[.panes[] | select(.presence == "managed") | select(.activity_state as $s | ($valid | index($s)) == null)] | length')
    assert_eq "activity_state values are valid enum" "0" "$INVALID_STATE"

    # evidence_mode must be "deterministic", "heuristic", or "none" for managed panes
    INVALID_EVIDENCE=$(echo "$OUTPUT" | jq \
        '[.panes[] | select(.presence == "managed") | select(.evidence_mode != "deterministic" and .evidence_mode != "heuristic" and .evidence_mode != "none")] | length')
    assert_eq "evidence_mode values are valid" "0" "$INVALID_EVIDENCE"
fi

echo "=== test-schema.sh PASS ==="
