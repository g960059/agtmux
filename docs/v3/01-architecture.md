# Architecture

## Decision Summary

1. Product の核心的価値は **agent 状態推定の正確性** にある
2. Terminal rendering は xterm.js で解決済み（VS Code, Hyper, Tabby 等で実証）
3. MVP 戦略: **CLI-first で状態推定を検証 → Desktop は後から接続**
4. **State engine が MVP gate**: 状態推定が不正確ならリリースしない
5. **TerminalBackend trait でターミナル環境を抽象化** — tmux は最初の実装だが唯一ではない

## Design Principles

1. **Backend-agnostic core** — state engine, adapters, attention derivation は特定の terminal multiplexer を知らない
2. **Trait-based composition** — provider capabilities を細粒度の trait で表現し、必要なものだけ実装する
3. **Type-safe enums over strings** — SourceType, ActivityState, Provider はすべて enum
4. **Library-first** — `agtmux-core` は IO 実装を含まない再利用可能な library

## Background

### Product 価値の再定義

```
MVP 価値の分布:
  状態推定の正確性  ████████████████████  80%  ← 不正確なら価値ゼロ
  terminal 表示     ███                    10%  ← xterm.js で解決済み
  見た目/操作性      ██                    10%  ← 後から改善可能
```

### クロスプラットフォーム戦略

| Platform | Daemon | Client |
|----------|--------|--------|
| macOS | ローカル実行 | TUI / tmux-status / Tauri |
| Windows | WSL or リモート | Tauri |
| iPhone | リモート | Tauri (WebSocket) |
| ブラウザ | リモート | Web app (将来) |

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  agtmux-core (library, no I/O)                                      │
│                                                                     │
│  types:     ActivityState, Evidence, Provider, PaneMeta, ...        │
│  engine:    Engine::resolve(&[Evidence]) → ResolvedActivity         │
│  adapt:     trait ProviderDetector, trait EvidenceBuilder, ...      │
│  attn:      derive_attention_state() → AttentionResult              │
│  backend:   trait TerminalBackend (abstract terminal environment)    │
│  source:    trait StateSource (abstract signal source)               │
└─────────────────────────────────────────────────────────────────────┘
         │ depends on
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│  agtmux-tmux (TerminalBackend impl for tmux)                        │
│  control_mode.rs, pipe_pane.rs, observer.rs, executor.rs            │
└─────────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│  agtmux-daemon (binary: daemon + CLI)                               │
│                                                                     │
│  Sources ──→ mpsc ──→ Orchestrator ──→ broadcast ──→ Clients        │
│  (Hook, Api, File, Poller)   (Engine + Attention)   (status/tui/…)  │
│                                                                     │
│  server.rs (Unix socket + WebSocket, JSON-RPC 2.0)                  │
│  store.rs (SQLite persistence)                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Core Design Decisions

1. **Engine は Evidence のみ受け取る** — PaneMeta を知らない pure scorer
2. **Orchestrator が PaneMeta → Evidence 変換を担当** — detectors + builders 経由
3. **TOML 宣言的 provider 定義** — signal→evidence mapping を `include_str!` で compile-time 埋め込み
4. **Async pipeline with channels** — `tokio::select!` + mpsc/broadcast

## TerminalBackend Trait

daemon のコアロジックは特定の terminal multiplexer に依存しない。

```rust
// agtmux-core/src/backend.rs
pub trait TerminalBackend: Send + Sync {
    fn list_panes(&self) -> Result<Vec<RawPane>>;
    fn capture_pane(&self, pane_id: &str) -> Result<String>;
    fn select_pane(&self, pane_id: &str) -> Result<()>;
}
```

Phase 1 では `TmuxBackend` のみ実装。将来 zellij や screen にも対応可能。

## Project Structure

```
agtmux/
├── Cargo.toml                  # workspace
├── crates/
│   ├── agtmux-core/            # 0 external I/O deps — library crate
│   │   └── src/
│   │       ├── lib.rs          # re-exports
│   │       ├── types/          # ActivityState, Evidence, Provider, PaneMeta, ...
│   │       ├── engine/         # Engine::resolve() — pure scoring
│   │       ├── adapt/          # trait ProviderDetector, EvidenceBuilder + impls
│   │       ├── attn/           # derive_attention_state()
│   │       ├── backend.rs      # trait TerminalBackend
│   │       └── source.rs       # trait StateSource, SourceEvent enum
│   │
│   ├── agtmux-tmux/            # deps: core + tokio — TerminalBackend impl
│   │   └── src/
│   │       ├── lib.rs          # TmuxBackend
│   │       ├── control_mode.rs # tmux -C parser
│   │       ├── pipe_pane.rs    # FIFO capture
│   │       ├── observer.rs     # list-panes diff
│   │       └── executor.rs     # tmux command execution
│   │
│   └── agtmux-daemon/          # deps: core, tmux + tokio, clap, ratatui, rusqlite
│       └── src/
│           ├── main.rs         # clap CLI
│           ├── orchestrator.rs # source → engine → broadcast loop
│           ├── server.rs       # Unix socket + WebSocket
│           ├── store.rs        # SQLite persistence
│           ├── sources/        # StateSource impls (hook, api, file, poller)
│           ├── tui.rs          # ratatui TUI
│           ├── status.rs       # one-shot output
│           └── tmux_status.rs  # tmux status line
│
├── providers/                  # TOML provider definitions
│   ├── claude.toml
│   ├── codex.toml
│   ├── gemini.toml
│   └── copilot.toml
│
├── fixtures/                   # テスト fixture JSON
└── desktop/                    # [Phase 5] Tauri
```

## Crate Dependency Rules

```
agtmux-core        (tokio/rusqlite/async 禁止。pure library)
    │
    ├── agtmux-tmux     (depends: core + tokio)
    │                    impl TerminalBackend for TmuxBackend
    │
    └── agtmux-daemon   (depends: core, tmux + tokio, clap, ratatui, rusqlite)
                         concrete wiring: TmuxBackend → Orchestrator → Clients
```

**依存ルール**:
- **agtmux-core**: tokio/rusqlite/async 禁止。pure library。serde, chrono, toml のみ許可
- **agtmux-tmux**: core のみに依存 + tokio
- **agtmux-daemon**: 全 crate に依存。binary crate
- **サイクル禁止**

**Key**: `agtmux-core` は trait 定義、型定義、pure engine logic のみ。IO 実装は `agtmux-tmux` と `agtmux-daemon` に分離。テストでは mock backend + in-memory store で完全に動作する。
