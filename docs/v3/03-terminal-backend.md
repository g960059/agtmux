# Terminal Backend Specification

## TerminalBackend Trait

`agtmux-core` で定義。daemon のコアロジックは特定の terminal multiplexer に依存しない。

```rust
// agtmux-core/src/backend.rs
pub trait TerminalBackend: Send + Sync {
    fn list_panes(&self) -> Result<Vec<RawPane>>;
    fn capture_pane(&self, pane_id: &str) -> Result<String>;
    fn select_pane(&self, pane_id: &str) -> Result<()>;
}

pub struct RawPane {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_name: String,
    pub current_cmd: String,
    pub pane_title: String,
    pub width: u16,
    pub height: u16,
    pub active: bool,
}
```

tmux は最初の実装だが、zellij や screen にも対応可能。

## TmuxBackend (`agtmux-tmux` crate)

### Control Mode Parser

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

**Rust 実装**: `Vec<u8>` で byte 蓄積し、boundary で `String::from_utf8_lossy()` を使う。

### Pipe-Pane

```bash
mkfifo /tmp/agtmux/pane-tap-<pid>-<pane_id>.fifo
tmux pipe-pane -t <pane_id> -O "exec cat > <fifo>"
```

- FIFO で raw terminal bytes を capture
- 16KB バッファ、non-blocking read
- detach 時: `tmux pipe-pane -t <pane_id>` で解除 + FIFO 削除

### Observer

```bash
tmux list-panes -a -F '#{session_id}:#{session_name}:#{window_id}:#{window_name}:#{pane_id}:#{pane_current_command}:#{pane_title}:#{pane_width}:#{pane_height}:#{pane_active}'
```

500ms 間隔でポーリング。前回との diff で add/remove/change イベントを生成。

### Executor

tmux コマンドの実行を抽象化:

```rust
// agtmux-tmux/src/executor.rs
pub struct TmuxExecutor { /* ... */ }

impl TmuxExecutor {
    pub fn run(&self, args: &[&str]) -> Result<String>;
}
```

## 将来の Backend 候補

| Backend | 接続方法 | Phase |
|---------|---------|-------|
| `TmuxBackend` | tmux control mode + pipe-pane | Phase 1 |
| `ZellijBackend` | Zellij IPC | 将来 |
| `NativePtyBackend` | forkpty() + VTE | 将来 |
