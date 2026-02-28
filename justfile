set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D clippy::dbg_macro -D clippy::todo -D clippy::unwrap_used -D clippy::undocumented_unsafe_blocks

test:
    cargo test --workspace --all-features --locked

verify: fmt lint test

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
    cargo run -p agtmux-runtime -- daemon {{ARGS}}

run-status *ARGS:
    cargo run -p agtmux-runtime -- status {{ARGS}}

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
    @test -x target/release/agtmux || test -x target/debug/agtmux || command -v agtmux >/dev/null || { echo "agtmux not built — run: cargo build -p agtmux-runtime"; exit 1; }
    @echo "[preflight-contract] OK"

e2e-contract: preflight-contract
    @cargo build -p agtmux-runtime --quiet
    @bash scripts/tests/e2e/contract/run-all.sh

# ── Layer 3: Detection E2E (real CLI required) ────────────────────────────
# Default timeout: 600s per run. Override: E2E_ONLINE_TIMEOUT=<seconds>

e2e-online: preflight-online
    @cargo build -p agtmux-runtime --quiet
    @PROVIDER="${PROVIDER:-claude}" \
     bash -c 'timeout "${E2E_ONLINE_TIMEOUT:-600}" bash scripts/tests/e2e/online/run-all.sh \
              || { ec=$?; [ $ec -eq 124 ] && echo "[ERROR] e2e-online timed out after ${E2E_ONLINE_TIMEOUT:-600}s" >&2; exit $ec; }'

e2e-online-claude: preflight-online
    @cargo build -p agtmux-runtime --quiet
    @PROVIDER=claude \
     bash -c 'timeout "${E2E_ONLINE_TIMEOUT:-600}" bash scripts/tests/e2e/online/run-all.sh \
              || { ec=$?; [ $ec -eq 124 ] && echo "[ERROR] e2e-online-claude timed out after ${E2E_ONLINE_TIMEOUT:-600}s" >&2; exit $ec; }'

e2e-online-codex: preflight-online
    @cargo build -p agtmux-runtime --quiet
    @PROVIDER=codex \
     bash -c 'timeout "${E2E_ONLINE_TIMEOUT:-600}" bash scripts/tests/e2e/online/run-all.sh \
              || { ec=$?; [ $ec -eq 124 ] && echo "[ERROR] e2e-online-codex timed out after ${E2E_ONLINE_TIMEOUT:-600}s" >&2; exit $ec; }'
