#!/usr/bin/env bash
# online/run-all.sh — Layer 3 Detection E2E: runs all online scenarios
#
# Environment variables:
#   PROVIDER           — provider name (claude | codex). Default: claude.
#   AGTMUX_BIN         — path to agtmux binary. Default: auto-detected.
#   E2E_SKIP_SCENARIOS — space-separated scenario names to skip (e.g. "provider-switch")
#
# Usage:
#   PROVIDER=claude bash scripts/tests/e2e/online/run-all.sh
#   PROVIDER=codex  bash scripts/tests/e2e/online/run-all.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"

PROVIDER="${PROVIDER:-claude}"
SKIP_SCENARIOS="${E2E_SKIP_SCENARIOS:-}"

# Validate provider adapter exists
ADAPTER_PATH="$SCRIPT_DIR/../providers/${PROVIDER}/adapter.sh"
if [ ! -f "$ADAPTER_PATH" ]; then
    echo "[ERROR] Provider adapter not found: $ADAPTER_PATH" >&2
    echo "        Available providers: claude, codex" >&2
    exit 1
fi

PASS_COUNT=0
FAIL_COUNT=0
FAILED_SCENARIOS=()

run_scenario() {
    local name="$1" script="$2"
    # Check skip list
    if printf ' %s ' "$SKIP_SCENARIOS" | grep -q " $name "; then
        echo "[SKIP] $name"
        return 0
    fi
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    if PROVIDER="$PROVIDER" bash "$script" 2>&1; then
        echo "[OK] $name"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo "[FAIL] $name"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        FAILED_SCENARIOS+=("$name")
    fi
}

echo "════════════════════════════════════════"
echo "Layer 3 Detection E2E (PROVIDER=${PROVIDER})"
echo "════════════════════════════════════════"

SCENARIOS_DIR="$SCRIPT_DIR/../scenarios"

run_scenario "single-agent-lifecycle"   "$SCENARIOS_DIR/single-agent-lifecycle.sh"
run_scenario "multi-agent-same-session" "$SCENARIOS_DIR/multi-agent-same-session.sh"

# same-cwd-multi-pane: defaults to codex (most relevant for T-124 regression)
if [ "$PROVIDER" = "codex" ]; then
    run_scenario "same-cwd-multi-pane" "$SCENARIOS_DIR/same-cwd-multi-pane.sh"
else
    echo "[SKIP] same-cwd-multi-pane (codex-specific, PROVIDER=${PROVIDER})"
fi

# provider-switch: only when both claude and codex are available
if command -v claude >/dev/null 2>&1 && command -v codex >/dev/null 2>&1; then
    run_scenario "provider-switch" "$SCENARIOS_DIR/provider-switch.sh"
else
    echo "[SKIP] provider-switch (requires both claude and codex CLIs)"
fi

echo ""
echo "════════════════════════════════════════"
printf "Detection E2E Results (PROVIDER=%s): %d passed, %d failed\n" "$PROVIDER" "$PASS_COUNT" "$FAIL_COUNT"
if [ ${#FAILED_SCENARIOS[@]} -gt 0 ]; then
    echo "Failed:"
    for s in "${FAILED_SCENARIOS[@]}"; do
        echo "  - $s"
    done
fi
echo "════════════════════════════════════════"

[ "$FAIL_COUNT" -eq 0 ]
