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
3. `specs/70-protocol-v3-wire-spec.md`  
wire実装時に必読。
4. `specs/71-quality-gates.md`  
gate閾値を確認する時に読む。
5. `specs/72-bootstrap-workspace.md`  
ゼロからworkspace作成時に読む。
6. `specs/73-notification-sink-extension.md`  
v2.1 で webhook 拡張を実装する時に読む。
7. `specs/74-fork-surface-map.md`  
fork 改造範囲と禁止範囲を確認する時に読む。
8. `specs/75-fork-hook-map-spike.md`  
Phase C 前に fork の hook 点を確定する時に読む。
9. `specs/76-output-hotpath-framing-policy.md`  
output hotpath を MessagePack のまま維持するか判定する時に読む。

## Decisions (Accepted ADRs)

1. `adr/ADR-0001-wezterm-fork-branch-strategy.md`
2. `adr/ADR-0002-ssh-tunnel-framing.md`
3. `adr/ADR-0003-notification-scope.md`
4. `adr/ADR-0004-wezterm-fork-integration-boundary.md`
5. `adr/ADR-0005-fork-source-integration-model.md`

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
