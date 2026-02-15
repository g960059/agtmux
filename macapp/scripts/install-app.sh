#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

INSTALL_DIR="${1:-$HOME/Applications}"
APP_NAME="AGTMUXDesktop.app"

"$SCRIPT_DIR/package-app.sh"

mkdir -p "$INSTALL_DIR"
rm -rf "$INSTALL_DIR/$APP_NAME"
cp -R "$MACAPP_ROOT/dist/$APP_NAME" "$INSTALL_DIR/$APP_NAME"

echo "Installed: $INSTALL_DIR/$APP_NAME"
echo "Open with: open \"$INSTALL_DIR/$APP_NAME\""
