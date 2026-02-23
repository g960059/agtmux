# Execution Plan

## Phase 0: State Engine Foundation

**Goal**: 確率的状態推定エンジンを Rust で新規設計。fixture テストで dev-gate 精度を達成。

Go POC のアルゴリズム構造 (evidence scoring + precedence + TTL) は参考にするが、
weight/confidence 数値はテスト駆動で独自に決定する。

**実装**:

| Module | Crate | File |
|--------|-------|------|
| ActivityState, Evidence, Provider, PaneMeta, SourceType | `agtmux-core` | `src/types/` |
| Engine::resolve() | `agtmux-core` | `src/engine/` |
| TOML provider definition loader | `agtmux-core` | `src/adapt/loader.rs` |
| ProviderDetector + EvidenceBuilder traits | `agtmux-core` | `src/adapt/mod.rs` |
| 4 provider adapters (claude, codex, gemini, copilot) | `agtmux-core` | `src/adapt/providers/` |
| derive_attention_state() | `agtmux-core` | `src/attn/` |
| TerminalBackend trait (定義のみ) | `agtmux-core` | `src/backend.rs` |
| StateSource trait (定義のみ) | `agtmux-core` | `src/source.rs` |
| JSON fixture files | — | `fixtures/` |
| proptest invariant tests | `agtmux-core` | `tests/` |

**テスト**:
- JSON fixture-driven regression: `fixtures/` 以下の全 JSON
- `proptest` で不変条件:
  - unmanaged pane → 常に Unknown
  - Error evidence が threshold 以上 → 常に Error が勝つ
  - TTL 期限切れ evidence → 無視される
  - score は常に [0.0, 1.0] の範囲内
- accuracy benchmark: `cargo test --test accuracy`

**Gate**: 全 fixture パス + proptest 不変条件 + dev-gate 精度 (weighted F1 >= 0.88)

## Phase 1: tmux Bridge

**Goal**: tmux に接続、pane topology 取得 + terminal output stream。

**実装**:

| Module | Crate | File |
|--------|-------|------|
| TmuxBackend (impl TerminalBackend) | `agtmux-tmux` | `src/lib.rs` |
| tmux control mode parser | `agtmux-tmux` | `src/control_mode.rs` |
| pipe-pane FIFO capture | `agtmux-tmux` | `src/pipe_pane.rs` |
| list-panes observer | `agtmux-tmux` | `src/observer.rs` |
| tmux command executor | `agtmux-tmux` | `src/executor.rs` |

**注意点**:
- octal escape decoder は CJK/emoji で壊れやすい
- Rust 実装では `Vec<u8>` で byte 蓄積し、boundary で `String::from_utf8_lossy()` を使う
- CJK (3-byte UTF-8) / emoji (4-byte UTF-8) の octal escape corpus test を必須で追加
- pipe-pane FIFO のライフサイクル管理 (orphan FIFO 防止)

**Gate**: list-panes 正確、pipe-pane 100ms 以内、CJK/emoji パス

## Phase 2: Daemon + Sources + API

**Goal**: Background daemon が tmux を監視、Unix socket で状態配信。

**実装**:

| Module | Crate | File |
|--------|-------|------|
| Orchestrator (main loop) | `agtmux-daemon` | `src/orchestrator.rs` |
| PollerSource | `agtmux-daemon` | `src/sources/poller.rs` |
| HookSource (Claude hooks stdin JSON) | `agtmux-daemon` | `src/sources/hook.rs` |
| ApiSource (Codex app-server WebSocket) | `agtmux-daemon` | `src/sources/api.rs` |
| FileSource (session file kqueue watch) | `agtmux-daemon` | `src/sources/file.rs` |
| Unix socket + WebSocket server | `agtmux-daemon` | `src/server.rs` |
| SQLite persistence | `agtmux-daemon` | `src/store.rs` |
| clap CLI | `agtmux-daemon` | `src/main.rs` |

**Multi-layer source integration**:
- 起動時に各 provider の利用可能 source を probe
- Claude: hooks 設定確認 → HookSource、`~/.claude/` 読み取り確認 → FileSource
- Codex: app-server 検出 → ApiSource、notify 確認 → HookSource
- 全 provider: PollerSource を常時有効化（fallback）

**Gate**: daemon 起動 → `list_panes` API で正しい状態

## Phase 3: CLI Views (MVP)

**Goal**: 3つの CLI view + recording mode。

**実装**:

| Module | Crate | File |
|--------|-------|------|
| `agtmux status` (one-shot) | `agtmux-daemon` | `src/status.rs` |
| `agtmux tui` (ratatui live) | `agtmux-daemon` | `src/tui.rs` |
| `agtmux tmux-status` (status line) | `agtmux-daemon` | `src/tmux_status.rs` |
| `agtmux daemon --record` (JSONL) | `agtmux-daemon` | `src/store.rs` |

**Gate**:
- 実機で Claude + Codex の状態遷移確認
- attention < 3s
- false positive rate <= 20%

## Phase 4: Accuracy + Config

**Goal**: Release-gate 精度 + 宣言的 provider 設定。

**作業**:
- `agtmux label` (手動ラベリング TUI)
- `agtmux accuracy` (precision/recall/F1 report)
- Provider signal→evidence mapping の TOML による runtime 上書き
- CI accuracy gate

**Gate**: activity_weighted_f1 >= 0.95

## Phase 5: Tauri Desktop (将来)

**Goal**: daemon の WebSocket API に Tauri v2 + xterm.js client を接続。

**作業**:
- `desktop/` — Tauri v2 + React + xterm.js
- sidebar = TUI の Web 版 (state indicators, attention badges)
- terminal = xterm.js (WebSocket binary frames で raw bytes streaming)
