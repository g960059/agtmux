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

## Phase 3: Post-MVP Hardening — Pure-logic crate wiring ✅ COMPLETE
- Deliverables (all wired into runtime):
  - T-118: LatencyWindow → poll_tick SLO evaluation + `latency_status` API + path escaping fix
  - T-116: CursorWatermarks + InvalidCursorTracker → gateway cursor pipeline (advance_fetched/commit + recovery)
  - T-117: SourceRegistry → `source.hello`/`source.heartbeat`/`list_source_registry` + staleness check
  - T-115: TrustGuard → UDS admission gate (warn-only) + `daemon.info` + source.ingest schema extension
- Implementation order: T-118 → T-116 → T-117 → T-115 ("observability first" + "lifecycle before admission")
- Codex plan review: Go with changes (5 findings, all adopted — see 70_progress.md)
- Exit criteria:
  - `just verify` PASS (585 tests = 565 MVP + 20 Phase 3)
  - 4 pure-logic crates (66 existing tests) wired into runtime with 20 integration tests
  - TrustGuard warn-only (enforce deferred to Phase 4)

## Phase 3b: Codex App Server 実働線 ✅ COMPLETE
- Goal: Codex App Server → CLI の deterministic pane 表示を end-to-end で動作させる
- Implementation order: T-120 → T-119
- Deliverables:
  - T-120: Protocol fix + reliability (jsonrpc compliance, reconnection, mutex fix, health, dead code cleanup)
  - T-119: pane_id correlation (thread.cwd ↔ tmux pane cwd → pane-level deterministic detection)
- Exit criteria:
  - `codex app-server` を起動中に `agtmux list-panes` で Codex pane が `signature_class: deterministic` と表示される
  - App Server プロセス kill 後、backoff 再接続で自動復旧する
  - `just verify` PASS
  - `just test-source-codex` で App Server 経由の evidence flow が確認できる

## Phase 4: Hardening Wave 2 (ops/security) — TODO
- Scope (default deferred):
  - TrustGuard enforce mode (promote warn-only → -32403 error on rejection)
  - supervisor strict readiness/backoff/hold-down
  - ops guardrail manager + `list_alerts`
  - Persistence (SQLite) for DaemonState
  - Multi-process extraction (separate source servers)

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
