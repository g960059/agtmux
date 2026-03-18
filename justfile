set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

fmt:
    cargo fmt --all -- --check

# Auto-fix formatting (use before committing)
fmt-fix:
    cargo fmt --all

# Release: bump version, verify all checks pass, tag, and push.
# Usage: just release 0.1.3
release VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== Releasing v{{VERSION}} ==="

    # 1. Clean working tree required
    if [ -n "$(git status --porcelain)" ]; then
        echo "ERROR: uncommitted changes present. Commit or stash first."
        exit 1
    fi

    # 2. Bump workspace version in Cargo.toml
    sed -i.bak 's/^version = ".*"/version = "{{VERSION}}"/' Cargo.toml && rm -f Cargo.toml.bak
    cargo update --workspace

    # 3. Verify BEFORE tagging — catches fmt/clippy/test failures locally
    just verify

    # 4. Commit, tag, push
    git add Cargo.toml Cargo.lock
    git commit -m "chore: bump version to {{VERSION}}"
    git tag "v{{VERSION}}"
    git push origin main
    git push origin "v{{VERSION}}"
    echo "=== Released v{{VERSION}} ==="

# Lint: matches CI exactly (cargo clippy --workspace -- -D warnings).
# Extra project-specific lints are layered on top.
lint:
    cargo clippy --workspace -- -D warnings -D clippy::dbg_macro -D clippy::todo -D clippy::unwrap_used -D clippy::undocumented_unsafe_blocks

test:
    cargo test --workspace --all-features --locked

verify: fmt lint test

# Install the project's pre-commit hook into the local .git/hooks/ directory.
# Run once after cloning: just install-hooks
install-hooks:
    cp scripts/pre-commit.sh .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    @echo "pre-commit hook installed"

preflight-online:
    @echo "[preflight] tmux availability"
    @command -v tmux >/dev/null || { echo "tmux not found"; exit 1; }
    @tmux -V >/dev/null
    @echo "[preflight] codex CLI + auth"
    @command -v codex >/dev/null || { echo "codex CLI not found"; exit 1; }
    @if codex_auth_output="$$(codex login status 2>&1)"; then \
      if [ -n "$$(printf '%s' "$$codex_auth_output" | tr -d '[:space:]')" ]; then \
        echo "codex auth: OK"; \
      else \
        echo "codex auth: empty output (fail-closed)"; \
        exit 1; \
      fi; \
    elif [ -n "${OPENAI_API_KEY:-}" ]; then \
      echo "codex auth: OPENAI_API_KEY detected"; \
    else \
      echo "codex auth: missing (run 'codex login status' or set OPENAI_API_KEY)"; \
      exit 1; \
    fi
    @echo "[preflight] claude CLI + auth"
    @command -v claude >/dev/null || { echo "claude CLI not found"; exit 1; }
    @if claude_auth_output="$$(claude auth status 2>&1)"; then \
      if [ -n "$$(printf '%s' "$$claude_auth_output" | tr -d '[:space:]')" ]; then \
        echo "claude auth: OK"; \
      else \
        echo "claude auth: empty output (fail-closed)"; \
        exit 1; \
      fi; \
    elif [ -n "${ANTHROPIC_API_KEY:-}" ]; then \
      echo "claude auth: ANTHROPIC_API_KEY detected"; \
    else \
      echo "claude auth: missing (run 'claude auth status' or set ANTHROPIC_API_KEY)"; \
      exit 1; \
    fi
    @echo "[preflight] network"
    @curl -fsS --max-time 5 https://api.github.com/zen >/dev/null || { echo "network check failed"; exit 1; }

test-source-codex:
    @if [ -f scripts/tests/test-source-codex.sh ]; then \
      just preflight-online; \
      bash scripts/tests/test-source-codex.sh; \
    else \
      echo "TODO: add scripts/tests/test-source-codex.sh"; \
    fi

test-source-claude:
    @if [ -f scripts/tests/test-source-claude.sh ]; then \
      just preflight-online; \
      bash scripts/tests/test-source-claude.sh; \
    else \
      echo "TODO: add scripts/tests/test-source-claude.sh"; \
    fi

test-source-poller:
    @if [ -f scripts/tests/test-source-poller.sh ]; then \
      bash scripts/tests/test-source-poller.sh; \
    else \
      echo "TODO: add scripts/tests/test-source-poller.sh"; \
    fi

poller-gate:
    cargo test -p agtmux-source-poller integration_fixture_gate -- --nocapture

run-daemon *ARGS:
    cargo run -p agtmux -- daemon {{ARGS}}

run-status *ARGS:
    cargo run -p agtmux -- status {{ARGS}}

test-e2e-status:
    @bash scripts/tests/test-e2e-status.sh

test-e2e-batch:
    @bash scripts/tests/run-e2e-batch.sh

test-e2e-matrix:
    @bash scripts/tests/run-e2e-matrix.sh

# ── Layer 2: Contract E2E (no real CLI needed) ────────────────────────────

preflight-contract:
    @echo "[preflight-contract] tmux"
    @command -v tmux  >/dev/null || { echo "tmux not found"; exit 1; }
    @tmux -V >/dev/null
    @echo "[preflight-contract] socat or python3 (for UDS injection)"
    @command -v socat >/dev/null || command -v python3 >/dev/null || command -v python >/dev/null || { echo "socat or python3 required (brew install socat)"; exit 1; }
    @echo "[preflight-contract] jq"
    @command -v jq    >/dev/null || { echo "jq not found (brew install jq)"; exit 1; }
    @echo "[preflight-contract] agtmux binary"
    @test -x target/release/agtmux || test -x target/debug/agtmux || command -v agtmux >/dev/null || { echo "agtmux not built — run: cargo build -p agtmux"; exit 1; }
    @echo "[preflight-contract] OK"

e2e-contract: preflight-contract
    @cargo build -p agtmux --quiet
    @AGTMUX_BIN=target/debug/agtmux bash scripts/tests/e2e/contract/run-all.sh

preflight-fake-live: preflight-contract
    @echo "[preflight-fake-live] OK"

e2e-fake-live: preflight-fake-live
    @cargo build -p agtmux --quiet
    @AGTMUX_BIN=target/debug/agtmux bash scripts/tests/e2e/fake-live/run-all.sh

verify-deterministic: poller-gate e2e-contract e2e-fake-live

# ── Layer 3: Detection E2E (real CLI required) ────────────────────────────
# Default timeout: 600s per run. Override: E2E_ONLINE_TIMEOUT=<seconds>

_timeout_cmd := if `command -v gtimeout 2>/dev/null || command -v timeout 2>/dev/null || echo ""` != "" { `command -v gtimeout 2>/dev/null || command -v timeout 2>/dev/null` } else { "" }

_run_online PROV:
    #!/usr/bin/env bash
    set -euo pipefail
    TOUT="${E2E_ONLINE_TIMEOUT:-600}"
    TCMD="{{_timeout_cmd}}"
    if [ -n "$TCMD" ]; then
        PROVIDER="{{PROV}}" "$TCMD" "$TOUT" bash scripts/tests/e2e/online/run-all.sh \
            || { ec=$?; [ $ec -eq 124 ] && echo "[ERROR] e2e-online timed out after ${TOUT}s" >&2; exit $ec; }
    else
        PROVIDER="{{PROV}}" bash scripts/tests/e2e/online/run-all.sh
    fi

e2e-online: preflight-online
    @cargo build -p agtmux --quiet
    just _run_online "${PROVIDER:-claude}"

e2e-online-claude: preflight-online
    @cargo build -p agtmux --quiet
    just _run_online claude

e2e-online-codex: preflight-online
    @cargo build -p agtmux --quiet
    just _run_online codex
