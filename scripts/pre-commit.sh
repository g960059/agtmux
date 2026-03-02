#!/usr/bin/env bash
# Pre-commit quality gate — run by git on every commit.
# Installs via: just install-hooks
set -euo pipefail

# 1. Auto-fix formatting and re-stage changed files
cargo fmt --all 2>&1
changed=$(git diff --name-only)
if [ -n "$changed" ]; then
    echo "[pre-commit] rustfmt fixed formatting in:"
    echo "$changed" | sed 's/^/  /'
    git add $changed
fi

# 2. Clippy — must match CI (cargo clippy --workspace -- -D warnings)
echo "[pre-commit] clippy..."
cargo clippy --workspace -- -D warnings 2>&1
echo "[pre-commit] OK"
