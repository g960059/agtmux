# agtmux

tmux agent multiplexer — a tcmux-style CLI that shows which tmux panes are running AI agents (Claude Code, Codex) and what they are doing.

## Features

- Detects Claude Code and Codex panes automatically via multiple evidence sources (hooks, JSONL, App Server, heuristics)
- Shows activity state: Running / Idle / Waiting
- Distinguishes deterministic (high-confidence) vs heuristic (`~`) evidence
- `agtmux list-panes` — sidebar view grouped by session
- `agtmux list-windows` — session/window hierarchy (tcmux-style)
- `agtmux list-sessions` — one-line-per-session summary
- fzf integration for instant window/session switching

## Install

```bash
cargo build --release
cp target/release/agtmux ~/.local/bin/
```

## Quick Start

### 1. Start the daemon

```bash
agtmux daemon
```

Or with logging:

```bash
AGTMUX_LOG=info agtmux daemon
```

### 2. Set up Claude Code hooks (optional, for higher-confidence Claude detection)

```bash
agtmux setup-hooks --scope user
```

### 3. View agent status

```bash
# Sidebar view (grouped by session)
agtmux list-panes

# Session/window hierarchy (tcmux-style)
agtmux list-windows

# One-line-per-session summary
agtmux list-sessions

# Raw JSON (all fields, for scripting)
agtmux list-panes --json | jq .

# Daemon summary
agtmux status
```

## agtmux list-panes

Grouped by session. `~` prefix = heuristic evidence (lower confidence). No marker = deterministic.

```
work
    Claude                          3m
  ~ Claude                          just now
    zsh
    zsh

personal
    Codex                           5m
```

Options:
- `--json`: raw JSON output (all fields)
- `--path`/`-p`: append current directory after each pane
- `--color=always|never|auto` (default: auto)

## agtmux list-windows

Session → window hierarchy. Window names only — tmux `@N` IDs are intentionally hidden.

```
work (2 windows — 2 Running, 1 Idle)
  dev — 2 Running
      Claude                Running  3m
      Codex                 Idle     1h
  tools — 1 Idle, 1 unmanaged
    ~ Claude                Idle     2h
      zsh

personal (1 window)
  main
      Codex                 Running  just now
```

Options:
- `--path`/`-p`: append current directory for each pane
- `--color=always|never|auto` (default: auto)

## agtmux list-sessions

One line per session — useful for quick overview and fzf session switching.

```
personal  1 window   1 agent  (1 Running)
work      2 windows  3 agents (2 Running, 1 Idle)  2 unmanaged
```

Options:
- `--color=always|never|auto` (default: auto)

## fzf Integration

The fzf recipes use `list-windows --color=never` to extract `session:window_name` pairs for targeting.

### Shell alias — window picker

Add to `~/.bashrc` or `~/.zshrc`:

```bash
# aw — pick a window by agent status and switch to it
alias aw='agtmux list-windows --color=never | \
  awk '"'"'/^[^ ]/{sess=$1} /^  [^ ]/{sub(/^  /,""); print sess ":" $1}'"'"' | \
  fzf | xargs tmux select-window -t'
```

Then type `aw` in any terminal to pick a window.

### One-liner

```bash
agtmux list-windows --color=never \
  | awk '/^[^ ]/{sess=$1} /^  [^ ]/{sub(/^  /,""); print sess ":" $1}' \
  | fzf | xargs tmux select-window -t
```

### Session switcher

```bash
agtmux list-sessions --color=never \
  | awk '{print $1}' \
  | fzf | xargs tmux switch-client -t
```

## Daemon Lifecycle

The daemon is a single process that embeds all components. It recovers from Codex App Server crashes automatically (exponential backoff, 5-min hold-down after 5 failures in 10 min).

If the daemon itself crashes, restart it:

```bash
agtmux daemon &
```

For production use, manage it with launchd or systemd.

## Commands

| Command | Description |
|---------|-------------|
| `agtmux daemon` | Start daemon (poll loop + UDS server) |
| `agtmux list-panes` | Sidebar view grouped by session (`--json` for raw JSON) |
| `agtmux list-windows` | Session/window/pane hierarchy (tcmux-style) |
| `agtmux list-sessions` | One-line-per-session summary |
| `agtmux status` | Daemon summary (pane count, source health) |
| `agtmux tmux-status` | One-line output for tmux status bar (`A:N U:M`) |
| `agtmux setup-hooks` | Write Claude Code hooks to settings.json |

## tmux Status Bar

Add to `~/.tmux.conf`:

```tmux
set -g status-right "#(agtmux tmux-status 2>/dev/null || echo 'A:? U:?') | %H:%M"
set -g status-interval 2
```
