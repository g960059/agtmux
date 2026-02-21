#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

UI_TEST_WORKDIR="${AGTMUX_UI_TEST_WORKDIR:-macapp}"
UI_TEST_COMMAND="${AGTMUX_UI_TEST_COMMAND:-swift test --filter AGTMUXDesktopUITests}"

if [[ "${AGTMUX_RUN_UI_TESTS:-0}" != "1" ]]; then
  cat <<'EOF'
UI tests require explicit opt-in.

Example:
  AGTMUX_RUN_UI_TESTS=1 ./scripts/ui-feedback/run-ui-tests.sh

Required permissions:
  - Accessibility
  - Screen Recording

Override (template setup):
  - AGTMUX_UI_TEST_WORKDIR
  - AGTMUX_UI_TEST_COMMAND
EOF
  exit 1
fi

if [[ -n "${SSH_CONNECTION:-}" || -n "${SSH_CLIENT:-}" || -n "${SSH_TTY:-}" ]]; then
  cat <<'EOF'
UI tests cannot run from SSH sessions (TCC is not applied).

Run from GUI login session Terminal.app or Xcode inside the VM.
EOF
  exit 2
fi

if [[ ! -d "$REPO_ROOT/$UI_TEST_WORKDIR" ]]; then
  cat <<EOF
error: AGTMUX_UI_TEST_WORKDIR does not exist: $REPO_ROOT/$UI_TEST_WORKDIR

This script is a template. Point it to your app workspace:
  AGTMUX_UI_TEST_WORKDIR=<dir> AGTMUX_UI_TEST_COMMAND='<command>' AGTMUX_RUN_UI_TESTS=1 ./scripts/ui-feedback/run-ui-tests.sh
EOF
  exit 3
fi

echo "Running UI tests in $UI_TEST_WORKDIR"
(
  cd "$REPO_ROOT/$UI_TEST_WORKDIR"
  eval "$UI_TEST_COMMAND"
)
