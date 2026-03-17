# ADR 20260225: Runtime Control Contracts (Supervisor, Ack, Registry, Binding Concurrency)

## Status
- Accepted

## Context
- 実装前レビューで以下の実装不能リスクが指摘された。
  - supervisor の起動/復旧契約不足
  - gateway->daemon ack の timeout/retry/idempotency 未定義
  - source registry lifecycle 不在
  - binding FSM の並行更新制御不足
  - ops guardrail manager の実体不足

## Decision
- Supervisor contract:
  - startup は source -> gateway -> daemon -> UI の依存順
  - readiness gate を通過しない限り次段起動しない（fail-closed）
  - restart は指数バックオフ（1s, x2, max 30s, jitter +-20%）
  - `5 failures / 10m` で hold-down 5m + escalate
- Ack contract:
  - delivery は `delivery_token` で管理
  - `ack_timeout=2s` で同一 token 再配送
  - `max_attempts=5` 超過で `ack_retry_exhausted` として source `degraded`
  - duplicate ack は `already_committed`（副作用なし）
- Source registry lifecycle:
  - state: `pending`, `active`, `stale`, `revoked`
  - `source.hello`/heartbeat timeout/operator action で遷移
  - connection admission は `active` source のみ受理
- Binding concurrency:
  - daemon projection loop の single-writer を採用
  - `state_version` CAS 更新 + conflict retry で巻き戻り防止
  - event ordering key は `(event_time, ingest_seq)` とする
- Ops guardrail:
  - warn/degraded/escalate の3段階を定義
  - `list_alerts` + alert ledger + logs へ同時出力

## Consequences
- Positive:
  - 障害時の挙動が deterministic になり、再現性のある運用が可能になる
  - 実装者の解釈差を抑制し、review基準を固定できる
- Negative / risks:
  - 制御面ロジックが増え、MVP初期実装コストが増加
  - 閾値調整（timeout/backoff/window）は運用データで再評価が必要

## Alternatives
- A: ack を best-effort に留める
  - 却下理由: cursor 一貫性と at-least-once 契約が崩れる
- B: binding 更新を lock-free で許容する
  - 却下理由: pane 再利用時に誤遷移/巻き戻りリスクが高い
- C: source registry を静的設定のみで運用する
  - 却下理由: socket rotation/recovery を fail-closed で扱えない

## Links
- Related docs:
  - `docs/20_spec.md` FR-039〜FR-047
  - `docs/30_architecture.md` Flow-011〜014
  - `docs/40_design.md` supervisor/ack/registry/binding sections
  - `docs/50_plan.md`, `docs/60_tasks.md`
