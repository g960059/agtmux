# Tasks Board (source of truth for execution; Orchestrator only)

## Status
- TODO / DOING / REVIEW / DONE / BLOCKED

## Rules
- Task IDs are stable. If splitting, use suffix (`T-010a`, `T-010b`).
- Every TODO task must declare `blocked_by`.
- REVIEW には Review Pack (`docs/85_reviews/RP-...`) を添付する。
- DONE は証跡（`just` 実行結果/test/review、必要時のみ commit/PR）を短く残す。
- online/e2e source test（codex/claude）を走らせる前に `just preflight-online` を必須実行する。

## TODO

### MVP Track (Phase 1-2; execute now)
- [ ] T-010 (P0) [US-005] v5 crate/workspace skeleton を作成
  - blocked_by: `none`
  - Output: `crates/agtmux-core-v5`, `crates/agtmux-gateway`, `crates/agtmux-daemon-v5`, `crates/agtmux-source-*`
  - Gates: `just verify`
- [ ] T-020 (P1) [US-001][US-002] tier resolver 実装 + unit/replay
  - blocked_by: `T-010`
  - Output: deterministic/fallback/re-promotion tests
  - Gates: resolver test suite pass
- [ ] T-011 (P1) [US-002][US-005] v4 poller logic を reusable crate 化
  - blocked_by: `T-010`
  - Output: poller core crate + compatibility tests
  - Gates: v4 fixture replay parity
- [ ] T-012 (P1) [US-005] v4 source health transition を reusable crate/module 化
  - blocked_by: `T-010`
  - Output: health transition module + contract tests
  - Gates: state transition tests pass
- [ ] T-013 (P1) [US-003] v4 title resolver を reusable 化して handshake title priority 実装
  - blocked_by: `T-010`
  - Output: handshake-aware title resolver + UI snapshot tests
  - Gates: canonical session title priority tests pass
- [ ] T-030 (P2) [US-001] codex appserver source server 実装
  - blocked_by: `T-010`
  - Output: `source.pull_events` for codex
  - Gates: `just preflight-online` + `just test-source-codex` + contract/integration tests
- [ ] T-031 (P2) [US-001] claude hooks source server 実装
  - blocked_by: `T-010`
  - Output: `source.pull_events` for claude hooks
  - Gates: `just preflight-online` + `just test-source-claude` + contract/integration tests
- [ ] T-032 (P2) [US-002] poller fallback server（v4再利用）実装
  - blocked_by: `T-011`
  - Output: poller source server + fallback tests
  - Gates: `just test-source-poller` + poller regression + fallback tests
- [ ] T-033 (P2) [US-002][US-005] poller baseline の再測定指標を確定
  - blocked_by: `T-032`
  - Output: v5 fallback quality spec（固定 dataset >=300 windows、weighted F1 >=0.85、waiting recall >=0.85）
  - Gates: spec review + fixture固定 + gate command で閾値判定 PASS
- [ ] T-040 (P2) [US-004] gateway basic aggregation/cursor/health 実装
  - blocked_by: `T-030,T-031,T-032`
  - Output: `gateway.pull_events`, `list_source_health`, single committed cursor progression
  - Gates: source multi-integration tests + crash/restart replay tests
- [ ] T-044 (P1/P3) [US-003] pane signature classifier v1 実装
  - blocked_by: `T-020`
  - Output: deterministic/heuristic/none classifier, heuristic weights（1.00/0.86/0.78/0.66）, title-only guard
  - Gates: unit tests（deterministic fields / title-only reject / wrapper guard）+ replay parity
- [ ] T-045 (P3) [US-003] signature hysteresis/no-agent demotion 実装
  - blocked_by: `T-044`
  - Output: idle stability `max(4s,2*interval)`, running promote 8s, running demote 45s, no-agent連続2回降格
  - Gates: integration tests（flap suppression, no-agent demotion, deterministic優先維持）
- [ ] T-050 (P3) [US-003] daemon v5 projection + client API
  - blocked_by: `T-020,T-040`
  - Output: `list_panes/list_sessions/state_changed/summary_changed`
  - Gates: integration tests + snapshot checks
- [ ] T-042 (P3) [US-003] pane-first binding state machine（MVP slice）実装
  - blocked_by: `T-044,T-050`
  - Output: `pane_instance` identity, generation更新, grace window handling, representative pane selection
  - Gates: pane reuse/migration integration tests + title representative determinism tests
- [ ] T-046 (P3) [US-003] signature fields API 露出
  - blocked_by: `T-044,T-050`
  - Output: `signature_class`, `signature_reason`, `signature_confidence`, compact `signature_inputs`
  - Gates: API contract tests + snapshot tests
- [ ] T-060 (P4) [US-003] supervisor + UI semantics (`agents` / unmanaged badge)
  - blocked_by: `T-030,T-031,T-032,T-050`
  - Output: startup/restart/shutdown behavior, UI labels
  - Gates: UX regression tests

### Post-MVP Backlog (Phase 3+; non-blocking for Phase 1-2)
- [ ] T-041 (P2/P3) [US-004] cursor contract hardening（ack進行 + invalid_cursor復旧）
  - blocked_by: `T-040`
  - Output: fetched/committed two-watermark, safe rewind（10m/10,000 events）, invalid_cursor streak/full-resync
  - Gates: ack/idempotency tests + rewind recovery tests
- [ ] T-043 (P3) [US-004] latency window instrumentation/SLO gate 実装
  - blocked_by: `T-040,T-050`
  - Output: rolling 10m p95 evaluator（min 200 events）, degraded alerts
  - Gates: 3連続 breach degraded tests
- [ ] T-047 (P2/P3) [US-004] UDS trust admission guard 実装
  - blocked_by: `T-040`
  - Output: peer uid check, source registry check, runtime nonce check
  - Gates: contract tests（peer_uid mismatch / registry miss / nonce mismatch）
- [ ] T-048 (P2/P3) [US-004] source.hello + registry lifecycle 実装
  - blocked_by: `T-047`
  - Output: source handshake API, lifecycle（pending/active/stale/revoked）, protocol mismatch reject
  - Gates: lifecycle tests + socket rotation/revoke tests
- [ ] T-049 (P3) [US-003][US-005] snapshot/restore 基盤 実装
  - blocked_by: `T-050`
  - Output: periodic/shutdown snapshot metadata, restore dry-run checker
  - Gates: restore dry-run tests + snapshot age assertions
- [ ] T-051 (P4) [US-005] observability alert routing 実装
  - blocked_by: `T-043,T-050`
  - Output: warn/degraded/escalate routing + diagnostics hooks + alert ledger sink
  - Gates: alert simulation tests + resolve policy tests
- [ ] T-052 (P4) [US-005] supervisor strict runtime contract 実装
  - blocked_by: `T-060`
  - Output: dependency readiness gate, exponential backoff, failure budget, hold-down
  - Gates: dependency-unready tests + hold-down/escalate tests
- [ ] T-053 (P3) [US-003] binding projection の並行更新制御 実装
  - blocked_by: `T-042,T-050`
  - Output: single-writer projection, `state_version` CAS, CAS conflict retry
  - Gates: concurrent event integration tests + rollback防止 tests
- [ ] T-070 (P5) [US-005] migration/canary/rollback runbook
  - blocked_by: `T-060`
  - Output: operator docs + rollback dry-run evidence
  - Gates: canary gate checklist
- [ ] T-071 (P5) [US-005] backup/restore runbook
  - blocked_by: `T-049`
  - Output: snapshot cadence, restore手順, 失敗時エスカレーション運用
  - Gates: restore dry-run evidence + review verdict `GO` 以上

## DOING
- [ ] (none)

## REVIEW
- [ ] (none)

## BLOCKED
- [ ] (none)

## DONE (keep short)
- [x] T-034 (P2) [US-001][US-004] source-specific test scripts を整備
  - Evidence: `scripts/tests/test-source-{codex,claude,poller}.sh` を追加し、`just preflight-online` / `just test-source-*` を実行
  - Notes: testは `/tmp/agtmux-e2e-*` の隔離git workspaceで実行し、完了時に tmux session/workspace/process を cleanup
- [x] T-035 (P2) [US-005] e2e reliability stress (10x + matrix) を実施
  - Evidence: `ITERATIONS=10 WAIT_SECONDS=30 PROMPT_STYLE=compact AGENTS=codex,claude just test-e2e-batch` -> codex 10/10, claude 10/10
- [x] T-009 (P0) [US-005] `just` ベースの local test/quality harness 初期整備
  - Evidence: root `justfile` 追加（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）
- [x] T-001 (P0) [US-005] docs-first baseline を v5 要件で再編
  - Evidence: `docs/00_router.md` 〜 `docs/90_index.md` をテンプレ準拠で再構成
- [x] T-002 (P0) [US-005] v5方針のユーザー確認を反映
  - Evidence: deterministic source固定、JSON-RPC over UDS、`agents` 英語固定、poller 85% baseline 位置づけ
- [x] T-003 (P0) [US-004][US-003] cursor/binding/latency 設計を docs へ反映
  - Evidence: FR-018〜FR-023 を docs に固定
- [x] T-004 (P0) [US-003] pane signature v1 設計を docs へ反映
  - Evidence: FR-024〜FR-031 を docs に固定
- [x] T-005 (P0) [US-005] review 指摘（品質/信頼境界/運用復旧）を docs 契約へ反映
  - Evidence: FR-032〜FR-038 を docs に固定
- [x] T-006 (P0) [US-005] review 指摘（supervisor/ack/registry/FSM並行制御）を docs 契約へ反映
  - Evidence: FR-039〜FR-047 を docs に固定
- [x] T-000 docs skeleton imported from template
  - Evidence: `~/Downloads/docs-first-template/docs` を基に初期構造作成済み
