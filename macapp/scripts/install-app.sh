#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

INSTALL_DIR="${1:-$HOME/Applications}"
APP_NAME="AGTMUXDesktop.app"

ensure_nerd_font() {
  if [[ "${AGTMUX_SKIP_FONT_INSTALL:-0}" == "1" ]]; then
    echo "Skipping Nerd Font install (AGTMUX_SKIP_FONT_INSTALL=1)"
    return
  fi
  if ! command -v brew >/dev/null 2>&1; then
    echo "Skipping Nerd Font install: Homebrew not found"
    return
  fi

  local cask_name="font-jetbrains-mono-nerd-font"
  if brew list --cask "$cask_name" >/dev/null 2>&1; then
    echo "Nerd Font already installed: $cask_name"
    return
  fi

  echo "Installing Nerd Font: $cask_name"
  brew tap homebrew/cask-fonts >/dev/null 2>&1 || true
  if HOMEBREW_NO_INSTALL_CLEANUP=1 brew install --cask "$cask_name" >/dev/null 2>&1; then
    echo "Installed Nerd Font: $cask_name"
  else
    echo "Warning: failed to install $cask_name (continuing)"
  fi
}

"$SCRIPT_DIR/package-app.sh"

ensure_nerd_font

mkdir -p "$INSTALL_DIR"
rm -rf "$INSTALL_DIR/$APP_NAME"
cp -R "$MACAPP_ROOT/dist/$APP_NAME" "$INSTALL_DIR/$APP_NAME"

echo "Installed: $INSTALL_DIR/$APP_NAME"
echo "Open with: open \"$INSTALL_DIR/$APP_NAME\""
