# Tasks Board (source of truth for execution; Orchestrator only)

## Status
- TODO / DOING / REVIEW / DONE / BLOCKED

## Rules
- Task IDs are stable. If splitting, use suffix (`T-010a`, `T-010b`).
- REVIEW には Review Pack (`docs/85_reviews/RP-...`) を添付する。
- DONE は証跡（`just` 実行結果/test/review、必要時のみ commit/PR）を短く残す。
- online/e2e source test（codex/claude）を走らせる前に `just preflight-online` を必須実行する。

## TODO
- [ ] T-010 (P0) [US-005] v5 crate/workspace skeleton を作成
  - Output: `crates/agtmux-core-v5`, `crates/agtmux-gateway`, `crates/agtmux-daemon-v5`, `crates/agtmux-source-*`
  - Gates: `just verify`
- [ ] T-011 (P1) [US-002][US-005] v4 poller logic を reusable crate 化
  - Output: poller core crate + compatibility tests
  - Gates: v4 fixture replay parity
- [ ] T-012 (P1) [US-005] v4 source health transition を reusable crate/module 化
  - Output: health transition module + contract tests
  - Gates: state transition tests pass
- [ ] T-013 (P1/P3) [US-003] v4 title resolver を reusable 化して handshake title priority 実装
  - Output: handshake-aware title resolver + UI snapshot tests
  - Gates: canonical session title priority tests pass
- [ ] T-020 (P1) [US-001][US-002] tier resolver 実装 + unit/replay
  - Output: deterministic/fallback/re-promotion tests
  - Gates: resolver test suite pass
- [ ] T-030 (P2) [US-001] codex appserver source server 実装
  - Output: `source.pull_events` for codex
  - Gates: `just preflight-online` + `just test-source-codex` + contract/integration tests
- [ ] T-031 (P2) [US-001] claude hooks source server 実装
  - Output: `source.pull_events` for claude hooks
  - Gates: `just preflight-online` + `just test-source-claude` + contract/integration tests
- [ ] T-032 (P2) [US-002] poller fallback server（v4再利用）実装
  - Output: poller source server + fallback tests
  - Gates: `just test-source-poller` + poller regression + fallback tests
- [ ] T-033 (P2) [US-002][US-005] poller baseline の再測定指標を確定
  - Output: v5 fallback quality spec（weighted F1 / waiting系 recall を含む）
  - Gates: spec review + accuracy fixture plan
- [ ] T-040 (P2) [US-004] gateway aggregation/cursor/health 実装
  - Output: `gateway.pull_events`, `list_source_health`
  - Gates: source multi-integration tests
- [ ] T-050 (P3) [US-003] daemon v5 projection + client API
  - Output: list/state/summary API
  - Gates: integration tests + snapshot checks
- [ ] T-060 (P4) [US-003] supervisor + UI semantics (`agents` / unmanaged badge)
  - Output: startup/restart/shutdown behavior, UI labels
  - Gates: UX regression tests
- [ ] T-070 (P5) [US-005] migration/canary/rollback runbook
  - Output: operator docs + rollback dry-run evidence
  - Gates: canary gate checklist

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
  - Notes: claude は `claude-sonnet-4-6` 固定で PASS、codex は `gpt-5.3-codex` + `medium` 固定（`codex exec --json`）で PASS、poller baseline も PASS
- [x] T-035 (P2) [US-005] e2e reliability stress (10x + matrix) を実施
  - Evidence: `ITERATIONS=10 WAIT_SECONDS=30 PROMPT_STYLE=compact AGENTS=codex,claude just test-e2e-batch` -> codex 10/10, claude 10/10
  - Notes: 短縮プロファイル（wait=30）での連続検証を確認、`just test-e2e-matrix`（fast-compact / conservative-strict 並列）も PASS
- [x] T-009 (P0) [US-005] `just` ベースの local test/quality harness 初期整備
  - Evidence: root `justfile` 追加（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）、`just --list` PASS
- [x] T-001 (P0) [US-005] docs-first baseline を v5 要件で再編
  - Evidence: `docs/00_router.md` 〜 `docs/90_index.md` をテンプレ準拠で再構成、v4/v3知見を反映
- [x] T-002 (P0) [US-005] v5方針のユーザー確認を反映
  - Evidence: deterministic source固定（Codex appserver / Claude hooks）、JSON-RPC over UDS、`agents` 英語固定、poller 85% baseline の位置づけを docs 反映
- [x] T-000 docs skeleton imported from template
  - Evidence: `~/Downloads/docs-first-template/docs` を基に初期構造作成済み
