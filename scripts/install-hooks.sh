#!/usr/bin/env bash
# Install git hooks for this repo.
# Run once after cloning: bash scripts/install-hooks.sh
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

install_hook() {
    local name="$1"
    local src="$REPO_ROOT/scripts/hooks/$name"
    local dst="$HOOKS_DIR/$name"
    cp "$src" "$dst"
    chmod +x "$dst"
    echo "installed: $dst"
}

install_hook pre-commit
echo "All hooks installed."
