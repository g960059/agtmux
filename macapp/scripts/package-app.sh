#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MACAPP_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

DIST_DIR="${1:-$MACAPP_ROOT/dist}"
APP_NAME="AGTMUXDesktop.app"
APP_BUNDLE="$DIST_DIR/$APP_NAME"
RUNTIME_BIN_DIR="$MACAPP_ROOT/.runtime/bin"

"$SCRIPT_DIR/build-local-binaries.sh" "$RUNTIME_BIN_DIR"

echo "Building Swift executable..."
cd "$MACAPP_ROOT"
if ! swift build -c release --product AGTMUXDesktop; then
  echo "" >&2
  echo "error: swift build failed." >&2
  echo "hint: If you only have CommandLineTools selected, switch to full Xcode:" >&2
  echo "  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
  exit 1
fi
EXECUTABLE_PATH="$MACAPP_ROOT/.build/release/AGTMUXDesktop"

if [[ ! -x "$EXECUTABLE_PATH" ]]; then
  echo "error: executable not found at $EXECUTABLE_PATH" >&2
  exit 1
fi

echo "Packaging app bundle..."
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources/bin"

cp "$EXECUTABLE_PATH" "$APP_BUNDLE/Contents/MacOS/AGTMUXDesktop"
cp "$RUNTIME_BIN_DIR/agtmuxd" "$APP_BUNDLE/Contents/Resources/bin/agtmuxd"
cp "$RUNTIME_BIN_DIR/agtmux-app" "$APP_BUNDLE/Contents/Resources/bin/agtmux-app"
chmod +x "$APP_BUNDLE/Contents/MacOS/AGTMUXDesktop" \
  "$APP_BUNDLE/Contents/Resources/bin/agtmuxd" \
  "$APP_BUNDLE/Contents/Resources/bin/agtmux-app"

cat > "$APP_BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>AGTMUXDesktop</string>
  <key>CFBundleIdentifier</key>
  <string>com.g960059.agtmux.desktop</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>AGTMUXDesktop</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP_BUNDLE" >/dev/null 2>&1 || true
fi

echo "Packaged: $APP_BUNDLE"
