#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ITERATIONS="${1:-3}"
DELAY_SECONDS="${AGTMUX_UI_LOOP_DELAY_SECONDS:-2}"
CAPTURE_DIR="${AGTMUX_UI_TEST_CAPTURE_DIR:-/tmp/agtmux-ui-captures}"

if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]] || [[ "$ITERATIONS" -lt 1 ]]; then
  echo "error: iterations must be a positive integer. got: $ITERATIONS" >&2
  exit 1
fi

cd "$MACAPP_ROOT"

for ((i = 1; i <= ITERATIONS; i++)); do
  echo "[ui-loop] run $i/$ITERATIONS"
  AGTMUX_RUN_UI_TESTS=1 \
  AGTMUX_UI_TEST_CAPTURE=1 \
  AGTMUX_UI_TEST_CAPTURE_DIR="$CAPTURE_DIR" \
  ./scripts/run-ui-tests.sh

  if [[ "$i" -lt "$ITERATIONS" ]]; then
    sleep "$DELAY_SECONDS"
  fi
done

echo ""
echo "[ui-loop] latest captures:"
if [[ -d "$CAPTURE_DIR" ]]; then
  ls -1t "$CAPTURE_DIR" | head -n 10 | sed "s|^|  $CAPTURE_DIR/|"
else
  echo "  (none)"
fi
