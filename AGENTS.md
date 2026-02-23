# AGTMUX v3

Rust daemon + CLI tools で terminal 上の AI agent 状態を推定・表示する。

## 設計文書

`docs/v3/README.md` を読み、作業中の Phase に必要なファイルだけ読む。

## 不変条件

1. State engine accuracy が MVP gate — 不正確ならリリースしない
2. Core logic (state engine, adapters, attention) は特定の terminal backend に依存しない
3. data plane と control plane を分離する

## Design Principles

1. **Backend-agnostic core** — TerminalBackend trait で terminal 環境を抽象化
2. **Trait-based composition** — provider capabilities を細粒度 trait で表現
3. **Type-safe enums** — SourceType, ActivityState 等は enum (string convention を使わない)
4. **Library-first** — agtmux-core は IO 実装を含まない再利用可能な library

## ファイル配置

- 設計文書: `docs/v3/`
- Rust ソース: `crates/`
- テスト fixture: `fixtures/`
