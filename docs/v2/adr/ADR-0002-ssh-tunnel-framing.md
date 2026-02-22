# ADR-0002: SSH Tunnel Framing Detail

Date: 2026-02-21  
Status: Accepted  
Owner: AGTMUX Core

## Context

v2 は local/ssh 共通で protocol v3 を利用する。  
未決定だったのは SSH トンネル上で追加フレーミングを入れるかどうか。

## Decision

SSH トンネル上では **追加フレーミングを導入しない**。  
local と同じ `protocol v3 (binary length-prefixed frame)` をそのまま透過させる。

運用ルール:

1. ssh 層は transport のみ（byte relay）
2. protocol は常に単一仕様
3. 接続再試行と backoff は transport 層で処理

## Consequences

メリット:

1. 二重仕様を回避できる
2. 実装・テスト・デバッグが単純化する
3. protocol 互換性検証を一本化できる

デメリット:

1. ssh transport 側の障害切り分けが別途必要

許容理由:

1. protocol 複雑化による不具合コストの方が大きいため

## Supersedes

`docs/v2/30-detailed-design.md` の Open Decision #2 を解決。
