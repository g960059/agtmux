#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN_DIR="$MACAPP_ROOT/.runtime/bin"

"$SCRIPT_DIR/build-local-binaries.sh" "$BIN_DIR"

export AGTMUX_DAEMON_BIN="$BIN_DIR/agtmuxd"
export AGTMUX_APP_BIN="$BIN_DIR/agtmux-app"

echo "AGTMUX_DAEMON_BIN=$AGTMUX_DAEMON_BIN"
echo "AGTMUX_APP_BIN=$AGTMUX_APP_BIN"
echo "Launching SwiftUI desktop app..."

cd "$REPO_ROOT/macapp"
if ! swift run AGTMUXDesktop; then
  echo "" >&2
  echo "error: swift run failed." >&2
  echo "hint: If you only have CommandLineTools selected, switch to full Xcode:" >&2
  echo "  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
  exit 1
fi
