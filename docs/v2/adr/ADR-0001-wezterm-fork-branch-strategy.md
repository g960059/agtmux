# ADR-0001: WezTerm Fork Branch Strategy

Date: 2026-02-21  
Status: Accepted  
Owner: AGTMUX Core

## Context

v2 は `wezterm-gui fork` 一本で進める。  
未決定だったのは、fork の追随運用を long-lived にするか、頻繁 rebase にするか。

## Decision

`long-lived fork/main` を採用し、upstream 取り込みは **隔週の更新ウィンドウ**で実施する。

運用ルール:

1. 通常開発は fork/main 基準で固定
2. upstream 取り込みは隔週でまとめて実施
3. 取り込み前後で replay regression gate を必須実行
4. 問題時は直前tagへ即時ロールバック

## Consequences

メリット:

1. 日次開発の再現性が高い
2. 破壊的追随で実装が止まりにくい
3. バグ切り分け範囲を限定できる

デメリット:

1. upstream 新機能反映が遅れる
2. 取り込み回で差分が大きくなりうる

許容理由:

1. v2初期は新機能追随より安定性を優先するため

## Supersedes

`docs/v2/30-detailed-design.md` の Open Decision #1 を解決。

実装境界（どこをforkで改造するか）は `ADR-0004` を参照。
