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

SUMMARY_LINE="$(grep -E 'Executed [0-9]+ tests, with [0-9]+ tests skipped and [0-9]+ failures' "$LOG_FILE" | tail -n 1 || true)"
SNAPSHOT_ERROR_COUNT="$(grep -c 'ui-snapshot-error:' "$LOG_FILE" || true)"
PARSED_EXECUTED=""
PARSED_SKIPPED=""
PARSED_FAILURES=""
if [[ -n "$SUMMARY_LINE" ]]; then
  if [[ "$SUMMARY_LINE" =~ Executed[[:space:]]+([0-9]+)[[:space:]]+tests, ]]; then
    PARSED_EXECUTED="${BASH_REMATCH[1]}"
  fi
  if [[ "$SUMMARY_LINE" =~ with[[:space:]]+([0-9]+)[[:space:]]+tests[[:space:]]+skipped ]]; then
    PARSED_SKIPPED="${BASH_REMATCH[1]}"
  fi
  if [[ "$SUMMARY_LINE" =~ and[[:space:]]+([0-9]+)[[:space:]]+failures ]]; then
    PARSED_FAILURES="${BASH_REMATCH[1]}"
  fi
fi

{
  echo "# AGTMUX UI Feedback Report"
  echo ""
  echo "- generated_at_utc: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo "- iterations: $ITERATIONS"
  echo "- status: $([ "$STATUS" -eq 0 ] && echo "PASS" || echo "FAIL")"
  echo "- capture_dir: $CAPTURE_DIR"
  if [[ -n "$PARSED_EXECUTED" ]]; then
    echo "- tests_executed: $PARSED_EXECUTED"
  fi
  if [[ -n "$PARSED_SKIPPED" ]]; then
    echo "- tests_skipped: $PARSED_SKIPPED"
  fi
  if [[ -n "$PARSED_FAILURES" ]]; then
    echo "- tests_failures: $PARSED_FAILURES"
  fi
  echo "- ui_snapshot_errors: $SNAPSHOT_ERROR_COUNT"
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
