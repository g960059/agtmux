#!/usr/bin/env bash
# contract/run-all.sh — run all Layer 2 contract e2e tests and report results
#
# Usage:
#   bash scripts/tests/e2e/contract/run-all.sh
#   AGTMUX_BIN=/path/to/agtmux bash scripts/tests/e2e/contract/run-all.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Binary resolution ──────────────────────────────────────────────────────

if [ -z "${AGTMUX_BIN:-}" ]; then
    # Try release build first, then debug, then PATH
    REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || echo "")"
    if [ -n "$REPO_ROOT" ] && [ -x "$REPO_ROOT/target/release/agtmux" ]; then
        export AGTMUX_BIN="$REPO_ROOT/target/release/agtmux"
    elif [ -n "$REPO_ROOT" ] && [ -x "$REPO_ROOT/target/debug/agtmux" ]; then
        export AGTMUX_BIN="$REPO_ROOT/target/debug/agtmux"
    elif command -v agtmux >/dev/null 2>&1; then
        export AGTMUX_BIN="agtmux"
    else
        echo "[error] agtmux binary not found. Build with 'cargo build -p agtmux-runtime' or set AGTMUX_BIN." >&2
        exit 1
    fi
fi
echo "[run-all] using agtmux: $AGTMUX_BIN ($("$AGTMUX_BIN" --version 2>/dev/null || echo 'unknown version'))"

# ── Test registry ─────────────────────────────────────────────────────────

TESTS=(
    "$SCRIPT_DIR/test-schema.sh"
    "$SCRIPT_DIR/test-claude-state.sh"
    "$SCRIPT_DIR/test-codex-state.sh"
    "$SCRIPT_DIR/test-waiting-states.sh"
    "$SCRIPT_DIR/test-error-state.sh"
    "$SCRIPT_DIR/test-list-consistency.sh"
    "$SCRIPT_DIR/test-multi-pane.sh"
    "$SCRIPT_DIR/test-freshness-fallback.sh"
)

# ── Runner ────────────────────────────────────────────────────────────────

PASS=0
FAIL=0
FAIL_NAMES=()

for test_script in "${TESTS[@]}"; do
    name="$(basename "$test_script")"
    echo ""
    echo "────────────────────────────────────────"
    echo "Running: $name"
    echo "────────────────────────────────────────"
    if bash "$test_script"; then
        PASS=$((PASS + 1))
        echo "[OK] $name"
    else
        FAIL=$((FAIL + 1))
        FAIL_NAMES+=("$name")
        echo "[FAIL] $name"
    fi
done

# ── Summary ───────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════"
echo "Contract E2E Results: $PASS passed, $FAIL failed"
if [ "${#FAIL_NAMES[@]}" -gt 0 ]; then
    echo "Failed:"
    for n in "${FAIL_NAMES[@]}"; do
        echo "  - $n"
    done
fi
echo "════════════════════════════════════════"

[ "$FAIL" -eq 0 ]
