# ADR 20260225: Operational Guardrails (Fallback Quality, Trust Boundary, Recovery)

## Status
- Accepted

## Context
- 実装前レビューで、以下の仕様不足が運用リスクとして指摘された。
  - poller fallback の受入基準が曖昧
  - `invalid_cursor` 復旧の数値契約が未固定
  - tombstone/grace の終端条件が不明
  - UDS trust boundary（接続元認証/認可）が不足
  - SLO はあるが失敗判定と運用遷移が弱い
  - backup/restore の必須運用が未定義

## Decision
- Poller fallback quality gate:
  - fixed dataset（`>= 300` labeled windows, Codex/Claude混在）で評価する。
  - release acceptance は `weighted F1 >= 0.85` かつ `waiting recall >= 0.85` を必須とする。
- Cursor recovery contract:
  - checkpoint: `30s` または `500 events` ごと。
  - safe rewind 上限: `min(10m, 10,000 events)`。
  - dedup retention: `rewind_window + 120s`（MVP既定 `>= 12m`）。
  - `invalid_cursor` が `60s` 内に3回連続した場合は full resync + warning。
- Binding lifecycle contract:
  - tombstone grace: `120s`。
  - purge: `tombstoned_at + 24h` 以降で遅延イベント未到達時に削除。
- UDS trust contract:
  - runtime dir `0700`, socket file `0600`。
  - peer credential（`SO_PEERCRED` / `getpeereid`）で `peer_uid == runtime_uid` を必須化。
  - source registry（`source_kind + socket_path + owner_uid`）不一致と runtime nonce 不一致は fail-closed。
- SLO operational contract:
  - rolling `10m` window、`>= 200 events/window` で評価。
  - 3連続 breach 時に `degraded` 遷移 + `slo_breach` 記録。
- Backup/restore contract:
  - `15m` periodic snapshot + shutdown snapshot を必須化。
  - canary 前 restore dry-run を必須化し、証跡を残す。

## Consequences
- Positive:
  - 実装時の解釈差を減らし、判定/復旧/運用を再現可能にできる。
  - fallback 品質、障害復旧、セキュリティ境界を gate 化できる。
- Negative / risks:
  - 実装対象（gateway/daemon/supervisor）の責務が増え、初期開発コストが上がる。
  - 数値閾値は運用データで再調整が必要になる可能性がある。

## Alternatives
- A: 数値契約を持たず運用チューニングで吸収
  - 却下理由: 実装後に GO/NO_GO 判定が主観化する。
- B: UDS を同一ユーザー前提の permission のみに依存
  - 却下理由: same-UID の偽イベント注入リスクを抑えきれない。
- C: tombstone を無期限保持
  - 却下理由: 状態肥大化と非決定性が増える。

## Links
- Related docs:
  - `docs/20_spec.md` FR-032〜FR-038
  - `docs/30_architecture.md` Flow-009/010, Security/Observability
  - `docs/40_design.md` UDS trust / Resolver / Binding / Latency / Backup
  - `docs/50_plan.md` Phase 2〜5
- Related tasks:
  - `docs/60_tasks.md` T-033, T-041, T-042, T-043, T-047, T-048, T-049, T-051, T-071
