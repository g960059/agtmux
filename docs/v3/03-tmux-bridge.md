# tmux Bridge Specification

## Control Mode Parser

`tmux -C attach-session` の出力を解析:

| Event | Format | 用途 |
|-------|--------|------|
| `%output` | `%output <pane-id> <octal-escaped-bytes>` | Terminal output |
| `%extended-output` | `%extended-output <pane-id> <age> <octal-escaped-bytes>` | Extended output |
| `%layout-change` | `%layout-change <window-id> <layout-string>` | Window layout |
| `%session-changed` | `%session-changed $<id> <name>` | Session change |
| `%window-add` | `%window-add @<id>` | Window added |
| `%exit` | `%exit [reason]` | Control mode exit |

Octal escape decoding: `\033` → ESC (0x1B) 等。CJK/emoji の multi-byte sequence に注意。

## Pipe-Pane

```bash
mkfifo /tmp/agtmux/pane-tap-<pid>-<pane_id>.fifo
tmux pipe-pane -t <pane_id> -O "exec cat > <fifo>"
```

- FIFO で raw terminal bytes を capture
- 16KB バッファ、non-blocking read
- detach 時: `tmux pipe-pane -t <pane_id>` で解除 + FIFO 削除

## Observer

```bash
tmux list-panes -a -F '#{session_id}:#{session_name}:#{window_id}:#{window_name}:#{pane_id}:#{pane_current_command}:#{pane_title}:#{pane_width}:#{pane_height}:#{pane_active}'
```

500ms 間隔でポーリング。前回との diff で add/remove/change イベントを生成。
