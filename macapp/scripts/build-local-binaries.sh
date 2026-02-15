#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT_DIR="${1:-$REPO_ROOT/macapp/.runtime/bin}"

mkdir -p "$OUT_DIR"

echo "Building AGTMUX binaries into: $OUT_DIR"
go build -o "$OUT_DIR/agtmux" "$REPO_ROOT/cmd/agtmux"
go build -o "$OUT_DIR/agtmuxd" "$REPO_ROOT/cmd/agtmuxd"
go build -o "$OUT_DIR/agtmux-app" "$REPO_ROOT/cmd/agtmux-app"
chmod +x "$OUT_DIR/agtmux" "$OUT_DIR/agtmuxd" "$OUT_DIR/agtmux-app"

echo "Done:"
echo "  $OUT_DIR/agtmux"
echo "  $OUT_DIR/agtmuxd"
echo "  $OUT_DIR/agtmux-app"
