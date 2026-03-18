#!/usr/bin/env bash
# fake-live/run-all.sh — run all fake-live tmux integration tests

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -z "${AGTMUX_BIN:-}" ]; then
    REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || echo "")"
    if [ -n "$REPO_ROOT" ] && [ -x "$REPO_ROOT/target/release/agtmux" ]; then
        export AGTMUX_BIN="$REPO_ROOT/target/release/agtmux"
    elif [ -n "$REPO_ROOT" ] && [ -x "$REPO_ROOT/target/debug/agtmux" ]; then
        export AGTMUX_BIN="$REPO_ROOT/target/debug/agtmux"
    elif command -v agtmux >/dev/null 2>&1; then
        export AGTMUX_BIN="agtmux"
    else
        echo "[error] agtmux binary not found. Build with 'cargo build -p agtmux' or set AGTMUX_BIN." >&2
        exit 1
    fi
fi

echo "[run-all] using agtmux: $AGTMUX_BIN ($("$AGTMUX_BIN" --version 2>/dev/null || echo 'unknown version'))"

TESTS=(
    "$SCRIPT_DIR/scenarios/same-pane-provider-switch.sh"
    "$SCRIPT_DIR/scenarios/shell-demotion-same-pane.sh"
    "$SCRIPT_DIR/scenarios/same-cwd-multi-pane-no-bleed.sh"
    "$SCRIPT_DIR/scenarios/daemon-restart-rebind.sh"
)

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

echo ""
echo "════════════════════════════════════════"
echo "Fake-Live E2E Results: $PASS passed, $FAIL failed"
if [ "${#FAIL_NAMES[@]}" -gt 0 ]; then
    echo "Failed:"
    for n in "${FAIL_NAMES[@]}"; do
        echo "  - $n"
    done
fi
echo "════════════════════════════════════════"

[ "$FAIL" -eq 0 ]
