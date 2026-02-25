# ADR 20260225: Cursor Contract, Pane-first Binding, Latency Budget

## Status
- Accepted

## Context
- レビューで以下3点の詰め不足が指摘された。
  - cursor/再送/重複排除の契約
  - pane再利用時の binding state machine
  - source->gateway->daemon の遅延予算定量化
- v5 は deterministic 優先 + poller fallback の2層設計を採るため、ここが曖昧だと再現性と運用性が落ちる。

## Decision
- Cursor contract:
  - source cursor は `fetched_cursor` / `committed_cursor` の二水位で管理する。
  - `committed_cursor` は daemon projection/persist 完了 ack 後にのみ前進させる。
  - 取り込みは at-least-once を前提とし、dedup key は `provider + session_key + event_id` で固定する。
  - `invalid_cursor` は safe rewind（checkpoint + dedup window）で自動復旧する。
- Binding model:
  - pane-first を採用し、正規キーは `pane_instance = (pane_id, generation, birth_ts)` とする。
  - `session_key` は pane への時系列 link として扱う（固定1:1にしない）。
  - 同一 `pane_id` の再利用時は generation 更新。旧実体は grace window 中 `tombstone` で保持する。
  - session tile 代表paneは最新 deterministic handshake を優先し、同点は latest activity で決定する。
- Latency budget:
  - deterministic path: `state_changed p95 <= 2.0s`
  - fallback degraded path: `p95 <= 5.0s`
  - hop budget(p95): source 0.6s / gateway 0.4s / daemon 0.6s / client 0.4s

## Consequences
- Positive:
  - 再起動・再送時の欠落/重複挙動が仕様で固定される。
  - pane再生成・移動・多重紐付けで誤結合しにくくなる。
  - 遅延悪化の原因を hop 単位で観測できる。
- Negative / risks:
  - `gateway.ack_delivery` を含む実装面の複雑度が上がる。
  - pane-instance 管理（generation/tombstone）のストレージ設計が増える。

## Alternatives
- A: cursor を受信時に即commit
  - 却下理由: daemon反映前クラッシュでイベント欠落リスクが高い。
- B: session-first で binding 管理
  - 却下理由: pane-first 要件（tmux pane観測中心）と衝突し、pane再利用時の追跡が不安定。
- C: 遅延SLOを p95 3.0s のまま維持
  - 却下理由: v5改善効果が不明瞭になり、pull連鎖の監視基準として弱い。

## Links
- Related docs:
  - `docs/20_spec.md` FR-018〜FR-023
  - `docs/30_architecture.md` Flow-006/007, Observability
  - `docs/40_design.md` Cursor/Binder/Latency sections
- Related tasks:
  - `docs/60_tasks.md` T-040, T-041, T-042, T-043
