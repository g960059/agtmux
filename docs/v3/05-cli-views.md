# CLI Views Specification

## `agtmux status` (one-shot)

```
$ agtmux status

SESSION: main (attached)
  %1  claude-code   ● running          12s ago
  %2  codex         ◉ waiting_input     3s ago  ← needs attention
  %3  bash          ○ unmanaged

SESSION: debug
  %4  claude-code   ◉ waiting_approval  8s ago  ← needs attention
  %5  vim           ○ unmanaged

Attention: 2 panes need your input
```

State indicators:
- `●` (green) = running
- `◉` (yellow) = waiting_input / waiting_approval
- `◈` (red) = error
- `○` (gray) = idle / unmanaged
- `◌` (dim) = unknown

## `agtmux tui` (ratatui)

```
┌─ AGTMUX ─────────────────────────────────────────┐
│                                                    │
│  SESSION: main                                     │
│  ┌──────────────────────────────────────────────┐  │
│  │ ● %1  claude-code    running         12s ago │  │
│  │ ◉ %2  codex          waiting_input    3s ago │  │
│  │ ○ %3  bash           unmanaged              │  │
│  └──────────────────────────────────────────────┘  │
│                                                    │
│  SESSION: debug                                    │
│  ┌──────────────────────────────────────────────┐  │
│  │ ◉ %4  claude-code    waiting_approval  8s ago│  │
│  └──────────────────────────────────────────────┘  │
│                                                    │
│  Attention: 2 panes                                │
│                                                    │
├────────────────────────────────────────────────────┤
│ j/k: move  Enter: select-pane  q: quit  r: refresh│
└────────────────────────────────────────────────────┘
```

機能:
- j/k でペイン間移動 (vim-style)
- Enter で `tmux select-pane -t <pane_id>` 実行
- フィルタ: `a` = all, `m` = managed only, `!` = attention only
- 自動更新 (daemon subscribe による push)
- 色: state に応じた ANSI カラー

## `agtmux tmux-status` (tmux status line)

```
$ agtmux tmux-status
●2 ◉1 ○3
```

意味: running=2, attention=1, other=3

tmux 設定:
```
# .tmux.conf
set -g status-right '#(agtmux tmux-status)'
set -g status-interval 2
```
