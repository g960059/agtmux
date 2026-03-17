# agtmux

Know which AI agent pane needs you — without switching to it.

agtmux monitors all your tmux panes and tells you which ones are running Claude Code, Codex, or Gemini, and what each is currently doing.

```
work
    Claude      [Running]          3m   Fix auth bug
    Claude      [WaitingApproval]  just now
    Codex       [WaitingInput]     5s
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

> **Note**: Windows is not supported (tmux is not available on Windows).

---

## Quick start

```bash
# 1. Start the background daemon
agtmux daemon

# 2. Register Claude Code hooks for precise state detection
agtmux setup-hooks --scope user

# 3. See what every pane is doing
agtmux ls
```

Hooks write to `~/.claude/settings.json`. Other tools' hook entries are preserved (surgical merge). To remove:

```bash
agtmux setup-hooks --unregister --scope user
```

---

## Project docs

- `docs/product/` — stable product intent and system shape
- `docs/decisions/` — ADRs and accepted design history
- `docs/runbooks/` — operator procedures
- `docs/research/` — dated, non-authoritative notes
- `docs/archive/` — historical specs, plans, tasks, and reviews

---

## Commands

### `agtmux ls` — pane list

Shows every pane, grouped by tmux session. `~` marks heuristic evidence (lower confidence); no prefix means deterministic (direct from the agent).

```
work
    Claude      [Running]          3m   Fix auth bug
    Claude      [WaitingApproval]  just now
    Codex       [WaitingInput]     5s
    zsh

personal
  ~ Claude      [Running]          42s
```

| Flag | Description |
|------|-------------|
| `--group=tree` | Grouped by session, one pane per line (default) |
| `--group=session` | One summary line per session |
| `--group=pane` | Flat list, all panes |
| `--icons` | Nerd Font icons |
| `--color=always\|never\|auto` | Color output (default: auto) |

---

### `agtmux ls --group=session` — session summary

One line per session. Useful for fzf session switching.

```
personal  1 window   1 agent  (1 Running)
work      2 windows  3 agents (1 Running, 1 WaitingApproval, 1 WaitingInput)
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
agtmux watch                    # 1s interval (default)
agtmux watch --interval 5
agtmux watch --session work     # filter to one session
```

---

### `agtmux wait` — script waiter

Blocks until agents reach a target state. Useful in shell automation and CI.

```bash
agtmux wait --idle              # wait until all managed agents are Idle
agtmux wait --no-waiting        # wait until no agent is waiting for input or approval
agtmux wait --idle --timeout 60 # timeout after 60 seconds
agtmux wait --idle --session work
```

Exit codes: `0` success · `1` timeout · `2` no managed panes · `3` daemon unavailable

---

### `agtmux bar` — status bar snippet

Compact one-liner for embedding in the tmux status bar.

```bash
agtmux bar           # A:3 U:2
agtmux bar --tmux    # tmux-format with color codes
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
agtmux json --health  # include source health status
```

---

### `agtmux daemon` — background daemon

Single process, single binary. Manages polling, source connections, and the UDS server for CLI clients.

```bash
agtmux daemon                    # foreground
AGTMUX_LOG=info agtmux daemon    # with logging
agtmux daemon &                  # background (launchd/systemd recommended)
```

Auto-replaces an already-running daemon on startup. Auto-recovers from source crashes. Codex JSONL source uses byte-offset tracking to avoid re-reading from the start.

---

### `agtmux setup-hooks` — Claude Code hook management

Registers (or removes) Claude Code hooks in `settings.json`. Hooks give agtmux precise state signals: `WaitingInput`, `WaitingApproval`, `Running`.

```bash
# Register hooks globally
agtmux setup-hooks --scope user

# Register for current project only
agtmux setup-hooks --scope project

# Check registration status
agtmux setup-hooks --check --scope user

# Remove agtmux hooks (preserves other tools' hooks)
agtmux setup-hooks --unregister --scope user
```

| Flag | Description |
|------|-------------|
| `--scope user` | Write to `~/.claude/settings.json` (default: `project`) |
| `--scope project` | Write to `.claude/settings.json` in current directory |
| `--check` | Exit 0 if all hooks registered, exit 1 if any missing |
| `--unregister` | Remove only agtmux hook entries (surgical — preserves all other hooks) |
| `--hook-script PATH` | Override auto-detected hook script path |

Hook entries use the `AGTMUX_HOOK_TYPE=` sentinel for identification. Running `setup-hooks` twice is safe (idempotent).

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
| Claude Code | Hooks (UDS) · JSONL transcript watcher | yes |
| Codex | JSONL session watcher | yes |
| Gemini | planned | yes |
| GitHub Copilot | planned | yes |

---

## How detection works

agtmux runs two independent detection layers simultaneously:

**Deterministic** — events reported directly by the agent:
- **Claude Code**: 11 hook types over a Unix domain socket (`PreToolUse`, `PostToolUse`, `Notification`, `Stop`, `PermissionRequest`, …), with JSONL transcript watcher as fallback
- **Codex**: JSONL session file watcher with FSM-based state transitions (`task_started`, `task_complete`, `entered_review_mode`)

**Heuristic** — pattern matching from tmux capture + process inspection:
- Always running as a background fallback
- Claude Code: includes braille spinner detection in pane title (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`) for hooks-free environments

When a deterministic source is fresh (< 3s old), heuristic evidence is suppressed. If a deterministic source goes stale or the daemon loses contact, heuristic takes over immediately and re-promotes automatically when it recovers — no restart needed.

Source priority:

| Provider | Priority order |
|----------|----------------|
| Claude Code | hooks → jsonl → poller |
| Codex | jsonl → poller |

---

## Activity states

| State | Meaning |
|-------|---------|
| `Running` | Agent is actively processing (tool use, generation) |
| `WaitingInput` | Agent finished; waiting for next user message |
| `WaitingApproval` | Permission dialog is showing |
| `Idle` | Session exists but no recent activity |
| `Unknown` | Agent detected but state not yet determined |

---

## Architecture

Rust workspace, single binary.

```
agtmux (single process)
  ├─ agtmux-core-v5                 Types, tier resolver, classifier (no IO)
  ├─ agtmux-tmux-v5                 tmux IO boundary
  ├─ agtmux-source-poller           Heuristic — pattern match from tmux capture
  ├─ agtmux-source-claude-hooks     Deterministic — Claude hook UDS receiver
  ├─ agtmux-source-claude-jsonl     Deterministic — Claude JSONL transcript watcher
  ├─ agtmux-source-codex-jsonl      Deterministic — Codex JSONL session watcher
  ├─ agtmux-source-codex-appserver  (legacy) Codex JSON-RPC App Server
  ├─ agtmux-gateway                 Source aggregation + cursor management
  └─ agtmux-daemon-v5               Resolver, read-model, UDS JSON-RPC server
```

---

## Development

```bash
just verify              # fmt + lint + test (run before every commit)
just e2e-contract        # contract tests: schema, state, consistency
just e2e-online          # provider end-to-end (requires preflight)
just preflight-online    # check tmux/codex/claude/network readiness
```

Provider pattern definitions: `providers/*.toml`
Architecture and design docs: `docs/`
