#!/usr/bin/env bash
# contract/test-provider-field.sh — provider field + extended JSON schema v1 validation
#
# Verifies fields that test-schema.sh does NOT check:
#   1. provider: "claude" for claude_hooks events (not null, not "ClaudeCode")
#   2. conversation_title: present in schema (null or string)
#   3. updated_at: non-null ISO timestamp for managed panes
#   4. age_secs: non-null non-negative integer for managed panes
#   5. git_branch: present in schema (null is ok)
#   6. current_path: non-null for managed panes
#   7. Unmanaged panes have provider=null and no activity_state
#
# Uses claude_hooks source.ingest — the only UDS-injectable source kind.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../harness/daemon.sh"
source "$SCRIPT_DIR/../harness/inject.sh"

register_cleanup

SESSION="e2e-provider-$$"
SOCKET="/tmp/agtmux-e2e-provider-$$/agtmuxd.sock"
INJECTOR_PID=""

echo "=== test-provider-field.sh ==="

# ── Setup: tmux session with 2 panes (managed + unmanaged) ──────────────

tmux new-session -d -s "$SESSION" -n main 2>/dev/null
tmux split-window -t "$SESSION:main" -h 2>/dev/null

PANE1=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1)
PANE2=$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | tail -1)

if [ -z "$PANE1" ] || [ -z "$PANE2" ] || [ "$PANE1" = "$PANE2" ]; then
    fail "could not get two distinct pane IDs"
fi
log "managed pane=$PANE1, unmanaged pane=$PANE2"

cleanup_tmux() {
    [ -n "$INJECTOR_PID" ] && kill "$INJECTOR_PID" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    daemon_stop
}
trap cleanup_tmux EXIT

daemon_start "$SOCKET" 500
sleep 1

# ── Inject claude_hooks events into PANE1 only ──────────────────────────

SESSION_ID="e2e-provider-sess-$$"
INJECTOR_PID=$(inject_claude_event_loop "$SOCKET" "tool_start" "$SESSION_ID" "$PANE1")
log "tool_start injector PID=$INJECTOR_PID for pane=$PANE1"

wait_for_agtmux_state "$SOCKET" "$PANE1" "presence" "managed" 30
wait_for_agtmux_state "$SOCKET" "$PANE1" "activity_state" "running" 30

# ── Scenario 1: provider field = "claude" for managed pane ───────────────

PROVIDER=$(jq_get "$SOCKET" "$PANE1" "provider")
assert_eq "Scenario 1: provider for claude_hooks pane" "claude" "$PROVIDER"

pass "Scenario 1: provider='claude' (not null, not 'ClaudeCode')"

# ── Scenario 2: updated_at is a non-null ISO timestamp ──────────────────

UPDATED_AT=$(jq_get "$SOCKET" "$PANE1" "updated_at")
if [ -z "$UPDATED_AT" ] || [ "$UPDATED_AT" = "null" ]; then
    fail "Scenario 2: updated_at is null for managed pane"
fi
# Validate ISO 8601 format (must contain T and Z or +)
if ! echo "$UPDATED_AT" | grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2}T'; then
    fail "Scenario 2: updated_at '$UPDATED_AT' is not ISO 8601"
fi
pass "Scenario 2: updated_at='$UPDATED_AT' (valid ISO timestamp)"

# ── Scenario 3: age_secs is non-null non-negative integer ────────────────

AGE_SECS=$(jq_get "$SOCKET" "$PANE1" "age_secs")
if [ -z "$AGE_SECS" ] || [ "$AGE_SECS" = "null" ]; then
    fail "Scenario 3: age_secs is null for managed pane"
fi
# Must be a non-negative integer
if ! echo "$AGE_SECS" | grep -qE '^[0-9]+$'; then
    fail "Scenario 3: age_secs '$AGE_SECS' is not a non-negative integer"
fi
# Freshly injected events should have age_secs < 30
if [ "$AGE_SECS" -gt 30 ]; then
    fail "Scenario 3: age_secs=$AGE_SECS (expected < 30 for fresh events)"
fi
pass "Scenario 3: age_secs=$AGE_SECS (non-negative, reasonable)"

# ── Scenario 4: conversation_title field exists (null is acceptable) ─────

FULL_JSON=$("$AGTMUX_BIN" --socket-path "$SOCKET" json 2>/dev/null)
HAS_CONVO_FIELD=$(echo "$FULL_JSON" | jq --arg p "$PANE1" \
    '[.panes[] | select(.pane_id==$p) | has("conversation_title")] | .[0]')
assert_eq "Scenario 4: conversation_title field exists" "true" "$HAS_CONVO_FIELD"

pass "Scenario 4: conversation_title field present in managed pane schema"

# ── Scenario 5: git_branch field exists (null is acceptable) ─────────────

HAS_GIT_FIELD=$(echo "$FULL_JSON" | jq --arg p "$PANE1" \
    '[.panes[] | select(.pane_id==$p) | has("git_branch")] | .[0]')
assert_eq "Scenario 5: git_branch field exists" "true" "$HAS_GIT_FIELD"

pass "Scenario 5: git_branch field present in managed pane schema"

# ── Scenario 6: current_path is non-null for managed pane ────────────────

CURRENT_PATH=$(jq_get "$SOCKET" "$PANE1" "current_path")
if [ -z "$CURRENT_PATH" ] || [ "$CURRENT_PATH" = "null" ]; then
    fail "Scenario 6: current_path is null for managed pane"
fi
pass "Scenario 6: current_path='$CURRENT_PATH' (non-null)"

# ── Scenario 7: unmanaged pane has provider=null ─────────────────────────

PANE2_PROVIDER=$(jq_get "$SOCKET" "$PANE2" "provider")
assert_eq "Scenario 7: unmanaged pane provider" "null" "$PANE2_PROVIDER"

PANE2_ACTIVITY=$(jq_get "$SOCKET" "$PANE2" "activity_state")
assert_eq "Scenario 7: unmanaged pane activity_state" "null" "$PANE2_ACTIVITY"

pass "Scenario 7: unmanaged pane has provider=null, activity_state=null"

# ── Scenario 8: all v1 schema fields present on managed pane ─────────────
# Exhaustive check: every field from cmd_json.rs pane_to_json_v1() must exist
V1_FIELDS="pane_id session_name session_id window_name window_id presence provider activity_state evidence_mode conversation_title current_path git_branch current_cmd updated_at age_secs"
for field in $V1_FIELDS; do
    HAS=$(echo "$FULL_JSON" | jq --arg p "$PANE1" --arg f "$field" \
        '[.panes[] | select(.pane_id==$p) | has($f)] | .[0]')
    assert_eq "v1 schema field '$field' present" "true" "$HAS"
done

pass "Scenario 8: all JSON schema v1 fields present on managed pane"

kill "$INJECTOR_PID" 2>/dev/null || true
INJECTOR_PID=""

echo "=== test-provider-field.sh PASS ==="
