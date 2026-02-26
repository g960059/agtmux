# Plan (mutable; keep it operational)

## Execution Policy (Mode B)
- Phase 1-2 は `[MVP]` 要件だけを実装ブロッカーとする。
- `[Post-MVP]` 要件は Phase 3+ の hardening backlog として維持する。
- 実装中に `[Post-MVP]` が必要と判明した場合のみ、タスクを昇格して着手する。

## Phase 0: Setup / Spec Freeze
- Deliverables:
  - `00/10/20/30/40/50/60/70/80/85/90` の整備
  - FR の `[MVP]` / `[Post-MVP]` タグ付け
  - `Main (MVP Slice)` / `Appendix (Post-MVP)` の設計分離
  - root `justfile`（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）整備
- Exit criteria:
  - Phase 1-2 実装に必要な仕様が `MVP` スライスだけで完結
  - Post-MVP が非ブロッカーであることを tasks/plan に明記

## Phase 1: Core MVP (types + resolver)
- Deliverables:
  - `agtmux-core-v5`（EvidenceTier, PanePresence, EvidenceMode, SourceEventV2）
  - tier winner resolver
  - pane signature classifier v1
  - v4 再利用ロジック抽出（poller core / source-health / title resolver）
  - fresh/stale/down/re-promotion unit tests
- Exit criteria:
  - deterministic priority と fallback/re-promotion が unit/replay で PASS
  - signature classifier（weights/guard/hysteresis）が PASS
  - related stories: US-001, US-002

## Phase 2: MVP Runtime Path (sources + gateway + daemon + runtime)
- Deliverables:
  - `agtmux-source-codex-appserver`
  - `agtmux-source-claude-hooks`
  - `agtmux-source-poller`
  - gateway basic pull aggregation（single committed cursor）
  - daemon projection + client API (`list_panes/list_sessions/state_changed/summary_changed`)
  - pane-first binding basic flow + handshake title priority
  - Cursor contract fix（source は caught up 時も `Some(cursor)` を返す）
  - `agtmux-tmux-v5` crate（tmux IO boundary + pane generation tracking）
  - `agtmux-runtime` binary crate（CLI + daemon + UDS server）
  - Poll loop wiring: tmux -> poller -> gateway -> daemon（unmanaged pane tracking + compaction 付き）
- Exit criteria:
  - provider priority/suppress/fallback integration tests PASS
  - codex/claude online tests は `just preflight-online` 後に PASS
  - poller fallback quality gate (`weighted F1>=0.85`, `waiting recall>=0.85`) PASS
  - `agtmux daemon` starts, polls tmux, populates projection（managed + unmanaged panes）
  - `agtmux status` connects via UDS and displays pane/session info
  - `just verify` passes with all 8 crates
  - related stories: US-001, US-002, US-003, US-004

## Phase 3: Hardening Wave 1 (only if needed)
- Scope (default deferred):
  - cursor two-watermark + ack idempotency + retry
  - invalid_cursor numeric recovery
  - binding concurrency hardening（single-writer/CAS）
  - latency rolling-window evaluator
- Promotion rule:
  - 実装/運用で再現した課題に紐づくものだけ着手

## Phase 4: Hardening Wave 2 (ops/security)
- Scope (default deferred):
  - UDS trust boundary（peer credential, nonce）
  - source registry lifecycle（pending/active/stale/revoked）
  - supervisor strict readiness/backoff/hold-down
  - ops guardrail manager + `list_alerts`

## Phase 5: Migration / Cutover
- Deliverables:
  - v4/v5 side-by-side runbook
  - canary plan + rollback plan
  - operator checklist
- Optional hardening:
  - backup/restore runbook（snapshot/restore）

## Risks / Mitigations
- Risk: docs 先行で実装が遅れる
  - Mitigation: `[MVP]` のみで Phase 1-2 を完了させる
- Risk: hardening 未実装で運用課題が残る
  - Mitigation: 課題が再現した時点で Post-MVP タスクを昇格する

## Rollout
1. Internal alpha: v4 と v5 を並走し比較
2. Canary: 限定ユーザーで v5 を既定化
3. Gradual default: 新規セッションを段階移行
4. Full cutover: v5 既定化、v4 は rollback path として維持

## Rollback
- runtime selector で v4 daemon path に即時戻せること
- migration は additive 優先で downgrade 可能にする
