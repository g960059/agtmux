#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$MACAPP_ROOT"

if [[ "${AGTMUX_RUN_UI_TESTS:-0}" != "1" ]]; then
  cat <<'EOF'
AGTMUXDesktopUITests は明示有効化が必要です。

実行例:
  AGTMUX_RUN_UI_TESTS=1 ./scripts/run-ui-tests.sh

必要な権限:
  - Accessibility
  - Screen Recording
EOF
  exit 1
fi

if [[ -n "${SSH_CONNECTION:-}" || -n "${SSH_CLIENT:-}" || -n "${SSH_TTY:-}" ]]; then
  cat <<'EOF'
UIテストは SSH セッションからは実行できません（TCC 権限が適用されません）。

VM の GUI ログインセッション内で Terminal.app か Xcode から実行してください。
EOF
  exit 2
fi

echo "Running AGTMUXDesktopUITests..."
swift test --filter AGTMUXDesktopUITests
