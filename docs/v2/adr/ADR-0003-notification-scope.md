# ADR-0003: Notification Channel Scope

Date: 2026-02-21  
Status: Accepted  
Owner: AGTMUX Core

## Context

v2 で attention 通知をどこまで提供するか（アプリ内のみ / 外部連携含む）が未決定だった。

## Decision

v2.0 の通知スコープは次に限定する。

1. アプリ内 attention queue
2. macOS 通知

Webhook は **v2.1 拡張**として実装する（設計上は拡張ポイントを先に確保）。

## Consequences

メリット:

1. v2.0 で通知精度とノイズ制御に集中できる
2. 運用価値を早く安定化できる

デメリット:

1. 初期リリースで Slack/Discord 等への即時連携は不可

許容理由:

1. 外部配信より、まず「誤通知を減らす」方が優先価値のため

## Follow-up (v2.1)

1. `NotificationSink` interface を追加
2. Webhook sink を実装
3. queue -> sink delivery policy を定義

## Supersedes

`docs/v2/30-detailed-design.md` の Open Decision #3 を解決。
