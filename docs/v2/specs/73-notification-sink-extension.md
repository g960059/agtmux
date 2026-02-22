# Notification Sink Extension (v2.1 Target)

Date: 2026-02-21  
Status: Planned  
Depends on: `../adr/ADR-0003-notification-scope.md`

## 1. Purpose

v2.0 では app内 + macOS 通知に限定し、v2.1 で webhook を追加するための拡張面を先に固定する。

## 2. Scope (v2.1)

1. Webhook sink（Slack/Discord/Generic）
2. delivery retry/backoff
3. event dedupe (`event_id` 単位)

## 3. Interface

`NotificationSink`:

1. `send(item: AttentionItem) -> Result`
2. `health() -> SinkHealth`
3. `name() -> String`

## 4. Delivery Policy

1. source は `attention_events` unread queue
2. at-least-once delivery
3. sink 側 5xx/timeout は retry
4. 4xx は permanent failure として dead-letter 記録

## 5. Non-goals

1. v2.0 への即時導入
2. 双方向同期（外部ACKで状態更新）
