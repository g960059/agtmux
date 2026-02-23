# AGTMUX v3

Rust daemon + CLI tools で terminal 上の AI agent 状態を推定・表示する。

## 設計文書

`docs/v3/README.md` を読み、作業中の Phase に必要なファイルだけ読む。

## 不変条件

1. State engine accuracy が MVP gate — 不正確ならリリースしない
2. Core logic (state engine, adapters, attention) は特定の terminal backend に依存しない
3. data plane と control plane を分離する

## Design Principles

1. **Backend-agnostic core** — `TerminalBackend` trait で terminal 環境を抽象化
2. **Trait-based composition** — provider capabilities を細粒度 trait で表現
3. **Type-safe enums** — `SourceType`, `ActivityState`, `Provider` 等は enum (string convention を使わない)
4. **Library-first** — `agtmux-core` は IO 実装を含まない再利用可能な library

## Architecture Overview

```
agtmux-core (library, no I/O)
├── types/     ActivityState, Evidence, Provider, PaneMeta, SourceType
├── engine/    Engine::resolve(&[Evidence]) → ResolvedActivity
├── adapt/     trait ProviderDetector, trait EvidenceBuilder, trait EventNormalizer
├── attn/      derive_attention_state() → AttentionResult
├── backend.rs trait TerminalBackend
└── source.rs  trait StateSource, SourceEvent enum

agtmux-tmux (TerminalBackend impl for tmux)
├── control_mode.rs, pipe_pane.rs, observer.rs, executor.rs

agtmux-daemon (binary: daemon + CLI)
├── orchestrator.rs  Sources → mpsc → Engine → broadcast → Clients
├── server.rs        Unix socket + WebSocket (JSON-RPC 2.0)
├── store.rs         SQLite persistence
├── sources/         StateSource impls (hook, api, file, poller)
├── tui.rs, status.rs, tmux_status.rs
```

## Core Design Decisions

1. **Engine は Evidence のみ受け取る** — PaneMeta を知らない pure scorer
2. **Orchestrator が PaneMeta → Evidence 変換を担当** — detectors + builders 経由
3. **TOML 宣言的 provider 定義** — `providers/*.toml` で signal→evidence mapping を定義
4. **Async pipeline** — `tokio::select!` + mpsc/broadcast channels

## Key Traits

```rust
// agtmux-core/src/backend.rs
pub trait TerminalBackend: Send + Sync {
    fn list_panes(&self) -> Result<Vec<RawPane>>;
    fn capture_pane(&self, pane_id: &str) -> Result<String>;
    fn select_pane(&self, pane_id: &str) -> Result<()>;
}

// agtmux-core/src/adapt/mod.rs
pub trait ProviderDetector: Send + Sync {
    fn id(&self) -> Provider;
    fn detect(&self, meta: &PaneMeta) -> Option<f64>;
}

pub trait EvidenceBuilder: Send + Sync {
    fn provider(&self) -> Provider;
    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence>;
}

pub trait EventNormalizer: Send + Sync {
    fn provider(&self) -> Provider;
    fn normalize(&self, signal: &RawSignal) -> Option<NormalizedState>;
}
```

## 依存ルール

- **agtmux-core**: tokio/rusqlite/async 禁止。pure library。serde, chrono, toml のみ許可
- **agtmux-tmux**: core のみに依存 + tokio
- **agtmux-daemon**: 全 crate に依存。binary crate
- **サイクル禁止**

## ファイル配置

- 設計文書: `docs/v3/`
- Rust ソース: `crates/`
- Provider 定義: `providers/`
- テスト fixture: `fixtures/`

## テスト戦略

- **Layer 1**: Unit fixtures (`fixtures/*.json`) — Engine に Evidence を渡して出力を検証
- **Layer 2**: Replay scenarios (`fixtures/scenarios/`) — 時系列 state transition
- **Layer 3**: proptest — 不変条件 (empty→Unknown, expired→Unknown, Error always wins, score ∈ [0,1])
- **Layer 4**: Live validation (Phase 3+) — `agtmux daemon --record` + `agtmux label` + `agtmux accuracy`
