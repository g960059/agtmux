# AGTMUX v2 Docs Index (Minimal Context)

Date: 2026-02-21
Status: Active

この `docs/v2` は、実装agentのコンテキスト消費を抑えるために、読む順番と必読範囲を固定している。

## Read Order (Required)

1. `10-product-charter.md`
2. `20-unified-design.md`
3. `30-detailed-design.md`
4. `40-execution-plan.md`

## Optional (When Needed)

1. `50-poc-learnings.md`  
POCで何が壊れたかを確認したい時だけ読む。
2. `60-ui-feedback-loop.md`  
UI検証ループ（権限/TCC/skip運用/レポート）を整備する時だけ読む。

## Historical References (Do Not Read By Default)

1. `references/90-phase28-restart-architecture.md`
2. `references/91-rust-rewrite-design.md`

## Usage Policy For Implementation Agents

1. まず必読4ファイルのみ読む
2. 追加で読む前に、必要性を1行で明確化する
3. `references/` は仕様衝突時の比較確認でのみ参照する

## Scope Snapshot

1. tmux-first
2. pane-first default + window-capable operations
3. `wezterm-gui fork` 一本
4. selected pane stream-only
5. adapter-first state/attention
