# Plan (mutable; keep it operational)

## Phase 0: Setup / Spec Freeze
- Deliverables:
  - `00/10/20/30/40/50/60/70/80/85/90` の整備
  - v5 契約（tier/freshness/API）固定
  - root `justfile`（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）整備
- Exit criteria:
  - architecture-level の未決が `20_spec.md` の Open Questions のみに限定される
  - local-first の日次検証が `just verify` で完結し、commit/PR 前提でない

## Phase 1: Core MVP (types + resolver)
- Deliverables:
  - `agtmux-core-v5`（EvidenceTier, PanePresence, EvidenceMode, SourceEventV2）
  - tier winner resolver 実装
  - v4 再利用ロジックの抽出（poller core / source-health transition / title resolver）
  - fresh/stale/down/re-promotion unit tests
- Exit criteria:
  - deterministic priority と fallback/re-promotion が unit/replay で PASS
  - 再利用ロジックが独立モジュールとして結合テストを持つ
  - Related stories: US-001, US-002

## Phase 2: Source Servers + Gateway
- Deliverables:
  - `agtmux-source-codex-appserver`
  - `agtmux-source-claude-hooks`
  - `agtmux-source-poller`（v4 logic 再利用）
  - `agtmux-gateway` cursor/pull aggregation
- Exit criteria:
  - provider priority/suppress/fallback integration tests PASS
  - codex/claude の online/e2e source tests は `just preflight-online` 実行後のみ実施される
  - source health API で各source状態を取得可能
  - Related stories: US-001, US-002, US-004

## Phase 3: Daemon v5 + Client API
- Deliverables:
  - gateway pull loop
  - read model projection + storage v2
  - `list_panes/list_sessions/state_changed/summary_changed`
  - handshake-aware title resolution（session tile canonical title priority）
- Exit criteria:
  - 状態変化が CLI/TUI で観測できる
  - `managed/unmanaged` は agent session 有無で判定され、`agents` 表示ルールが反映される
  - deterministic/heuristic 切替は evidence mode として表示・配信される
  - handshake 完了 pane で agent session name が優先表示される
  - Related stories: US-003

## Reuse Policy
- Reuse first:
  - v4 poller logic
  - v4 title resolution logic
  - v4 source health transition logic
- Re-architecture required:
  - v4 orchestrator composition
  - v4 persistence layout

## Phase 4: Supervisor + UX integration
- Deliverables:
  - runtime supervisor（起動順/再起動/終了）
  - TUI/GUI 起動パス統一
  - unmanaged badge 表示最終調整
- Exit criteria:
  - source crash を含む運用シナリオで UX 継続
  - restart/backoff/shutdown が検証済み
  - Related stories: US-003, US-005

## Phase 5: Migration / Cutover
- Deliverables:
  - v4/v5 side-by-side runbook
  - canary plan + rollback plan
  - operator checklist
- Exit criteria:
  - canary環境で gate 充足
  - rollback dry-run 成功
  - Related stories: US-005

## Risks / Mitigations
- source protocol churn
  - Mitigation: SourceEvent schema versioning + contract tests
- process count 増加による運用複雑化
  - Mitigation: supervisor 一元管理 + health visibility
- fallback quality の過大評価
  - Mitigation: fallback専用 accuracy gate を固定し、live labeling で定期測定

## Rollout
1. Internal alpha: v4 と v5 を並走し、同一セッションを比較
2. Canary: 限定ユーザーで v5 supervisor を既定化
3. Gradual default: 新規セッションを v5 側へ段階移行
4. Full cutover: v5 を既定化、v4 は rollback path として維持

## Rollback
- runtime selector で v4 daemon path に即時戻せること
- storage migration は additive に限定し downgrade 可能にする
- rollback 手順は canary 前に dry-run して証跡を残す
