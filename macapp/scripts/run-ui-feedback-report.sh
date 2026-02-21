#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ITERATIONS="${1:-1}"
CAPTURE_DIR="${AGTMUX_UI_TEST_CAPTURE_DIR:-/tmp/agtmux-ui-captures}"
REPORT_PATH="${AGTMUX_UI_REPORT_PATH:-/tmp/agtmux-ui-feedback-report-$(date -u +%Y%m%dT%H%M%SZ).md}"

LOG_FILE="$(mktemp -t agtmux-ui-loop-log.XXXXXX)"
trap 'rm -f "$LOG_FILE"' EXIT

cd "$MACAPP_ROOT"

set +e
./scripts/run-ui-loop.sh "$ITERATIONS" >"$LOG_FILE" 2>&1
STATUS=$?
set -e

{
  echo "# AGTMUX UI Feedback Report"
  echo ""
  echo "- generated_at_utc: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo "- iterations: $ITERATIONS"
  echo "- status: $([ "$STATUS" -eq 0 ] && echo "PASS" || echo "FAIL")"
  echo "- capture_dir: $CAPTURE_DIR"
  echo ""
  echo "## Latest Captures"
  if [[ -d "$CAPTURE_DIR" ]]; then
    captures="$(ls -1t "$CAPTURE_DIR"/*.png 2>/dev/null | head -n 12 || true)"
    if [[ -z "$captures" ]]; then
      echo "- (none)"
    else
      while IFS= read -r file; do
        [[ -z "$file" ]] && continue
        echo "- $file"
      done <<<"$captures"
    fi
  else
    echo "- (capture dir not found)"
  fi
  echo ""
  echo "## Command Output"
  echo ""
  echo '```text'
  cat "$LOG_FILE"
  echo '```'
} >"$REPORT_PATH"

echo "ui-feedback-report: $REPORT_PATH"
if [[ "$STATUS" -ne 0 ]]; then
  exit "$STATUS"
fi
