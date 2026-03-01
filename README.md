# agtmux

Know which AI agent pane needs you — without switching to it.

agtmux monitors all your tmux panes and tells you which ones are running Claude Code, Codex, or Gemini, and what each is currently doing.

```
work
    Claude      [Running]          3m   Fix auth bug
    Claude      [WaitingApproval]  just now
    Codex       [Idle]             1h
    zsh

personal
  ~ Claude      [Running]          42s
```

---

## Install

### macOS

```bash
brew install g960059/tap/agtmux
```

### Linux

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/g960059/agtmux/releases/latest/download/agtmux-installer.sh | sh
```

### From source (Rust)

```bash
cargo install --locked agtmux
```

### Uninstall

```bash
# Homebrew
brew uninstall agtmux

# Manual install
rm ~/.local/bin/agtmux
```

> **Note**: Windows is not supported (tmux is not available on Windows).

## Quick start

```bash
# 1. Start the background daemon
agtmux daemon

# 2. (Recommended) Install Claude Code hooks for precise detection
agtmux setup-hooks --scope user

# 3. See what every pane is doing
agtmux ls
```

---

## Commands

### `agtmux ls` — pane list

Shows every pane, grouped by session. `~` marks heuristic evidence (lower confidence); no prefix means deterministic (direct from the agent).

```
work
    Claude      [Running]          3m   Fix auth bug
    Claude      [WaitingApproval]  just now
    Codex       [Idle]             1h
    zsh

personal
  ~ Claude      [Running]          42s
```

| Flag | Description |
|------|-------------|
| `--group=window` | Break down by window |
| `--group=session` | One line per session |
| `--context=auto\|off\|full` | Show conversation context (default: auto = only when deterministic) |
| `--color=always\|never\|auto` | Color output (default: auto) |

---

### `agtmux ls --group=window` — window view

```
work (2 windows — 2 Running, 1 WaitingApproval)
  dev — 2 Running
      Claude      Running          3m   Fix auth bug
      Codex       Idle             1h
  review — 1 WaitingApproval, 1 unmanaged
      Claude      WaitingApproval  just now
      zsh

personal (1 window)
  main
    ~ Claude      Running          42s
```

---

### `agtmux ls --group=session` — session summary

One line per session — useful for overview and fzf session switching.

```
personal  1 window   1 agent  (1 Running)
work      2 windows  3 agents (2 Running, 1 Idle)  1 WaitingApproval
```

---

### `agtmux pick` — interactive window picker

Jump to any window with fzf. Requires fzf in `$PATH`.

```bash
agtmux pick              # all windows
agtmux pick --waiting    # only windows with agents waiting for input or approval
agtmux pick --dry-run    # print target without switching
```

---

### `agtmux watch` — live monitor

Refreshes in place, like `watch`.

```bash
agtmux watch               # 2s interval (default)
agtmux watch --interval 5
```

---

### `agtmux wait` — script waiter

Blocks until agents reach a target state. Useful in automation.

```bash
agtmux wait Idle       # wait until all managed agents are Idle
agtmux wait NoWaiting  # wait until no agent is waiting for input or approval
```

Exit codes: `0` success · `1` timeout · `2` no managed panes · `3` daemon unavailable

---

### `agtmux bar` — status bar snippet

Compact one-liner for embedding in the tmux status bar.

```bash
agtmux bar           # A:3 U:2
agtmux bar --tmux    # tmux-format with color
```

Add to `~/.tmux.conf`:

```tmux
set -g status-right "#(agtmux bar --tmux 2>/dev/null || echo 'A:?') | %H:%M"
set -g status-interval 2
```

---

### `agtmux json` — raw JSON

All fields. For scripting and external tooling.

```bash
agtmux json
agtmux json --health  # include source health
```

---

### `agtmux daemon` — background daemon

Single process, single binary. Manages polling, source connections, and the UDS server for CLI clients.

```bash
agtmux daemon                    # foreground
AGTMUX_LOG=info agtmux daemon    # with logging
agtmux daemon &                  # background (launchd/systemd recommended)
```

Auto-recovers from source crashes. Codex App Server restarts use exponential backoff (hold-down after repeated failures).

---

## Shell integration

### Jump to a waiting agent (recommended alias)

```bash
# Add to ~/.zshrc or ~/.bashrc
alias aww='agtmux pick --waiting'
```

### fzf session switcher

```bash
agtmux ls --group=session --color=never \
  | awk '{print $1}' \
  | fzf | xargs tmux switch-client -t
```

---

## Supported providers

| Provider | Deterministic sources | Heuristic |
|----------|-----------------------|-----------|
| Claude Code | Hooks (UDS) + JSONL transcript watcher | yes |
| Codex | App Server (JSON-RPC) | yes |
| Gemini | planned | yes |
| GitHub Copilot | planned | yes |

---

## How detection works

agtmux runs two independent detection layers simultaneously:

**Deterministic** — events reported directly by the agent:
- Claude Code: hook callbacks over UDS, or JSONL transcript files as fallback
- Codex: JSON-RPC App Server stream

**Heuristic** — pattern matching from tmux capture + process inspection, always running as a fallback.

When a deterministic source is fresh (< 3s old), heuristic evidence is suppressed. If a deterministic source goes stale or down, the heuristic tier takes over immediately and re-promotes automatically when it recovers — no restart needed.

Source priority per provider:

| Provider | Order |
|----------|-------|
| Claude Code | hooks > jsonl > poller |
| Codex | appserver > poller |

---

## Architecture

Rust workspace, single binary in MVP.

```
agtmux (single process)
  ├─ agtmux-core-v5                 Types, tier resolver, classifier (no IO)
  ├─ agtmux-tmux-v5                 tmux IO boundary
  ├─ agtmux-source-poller           Heuristic — pattern match from tmux capture
  ├─ agtmux-source-claude-hooks     Deterministic — Claude hook UDS receiver
  ├─ agtmux-source-claude-jsonl     Deterministic — JSONL transcript watcher
  ├─ agtmux-source-codex-appserver  Deterministic — Codex JSON-RPC App Server
  ├─ agtmux-gateway                 Source aggregation + cursor management
  └─ agtmux-daemon-v5               Resolver, read-model, UDS JSON-RPC server
```

---

## Development

```bash
just verify              # fmt + lint + test (gate before every commit)
just e2e-contract        # contract tests: schema, state, consistency
just e2e-online          # provider end-to-end (requires preflight)
just preflight-online    # check tmux/codex/claude/network readiness
```

Provider pattern definitions: `providers/*.toml`
Architecture and design docs: `docs/`
