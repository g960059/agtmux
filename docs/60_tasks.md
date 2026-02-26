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
- [ ] (none)

## DOING
- [ ] (none)

## REVIEW
- [ ] (none)

## BLOCKED
- [ ] (none)

## DONE (keep short)
- [x] T-107 (P1) [MVP] Detection accuracy + activity_state display
  - Evidence: Capture-based 4th detection signal (WEIGHT_POLLER_MATCH=0.78), stale title suppression (title-only + shell + no capture → None), per-pane activity_state + provider in list-panes output. Codex+Claude parallel review adopted (capture tokens tightened: `╭ Claude Code`/`codex>`, shell list expanded: nu/pwsh/tcsh/csh, capture_match wired through payload→poller_match, provider as Option, changed condition updated). `just verify` PASS (525 tests = 514 existing + 11 new).
- [x] T-106 (P1) test strategy + quality gates for runtime crates
  - Evidence: FakeTmuxBackend (mock TmuxCommandRunner) + 12 poll_tick integration tests + 4 build_pane_list unit tests = 16 new runtime tests. E2E smoke script (`just test-e2e-status`). `just verify` PASS (514 tests). `just test-e2e-status` PASS with live tmux.
- [x] T-105 (P1) CLI polish: tmux-status, socket targeting, --poll-interval-ms
  - Evidence: `agtmux tmux-status` outputs `A:4 U:13`. `--tmux-socket`, `AGTMUX_TMUX_SOCKET_PATH/NAME` env supported. `--poll-interval-ms` configurable.
- [x] T-104 (P0) UDS JSON-RPC server + client CLI
  - Evidence: UDS server (connection-per-request, dir 0700, socket 0600, stale cleanup). `agtmux status` connects and prints pane info. 3 methods: list_panes, list_sessions, list_source_health.
- [x] T-103 (P0) poll loop: tmux -> poller -> gateway -> daemon pipeline
  - Evidence: poll_loop.rs wires tmux → poller → gateway → daemon. Unmanaged panes tracked via last_panes + build_pane_list merge. Error recovery (log+skip on capture failure).
- [x] T-102 (P0) runtime skeleton: binary + CLI + daemon + logging
  - Evidence: `agtmux` binary with clap CLI (daemon/status/list-panes/tmux-status). tracing + tracing-subscriber. Signal handling (ctrl_c). `just verify` PASS with 8 crates.
- [x] T-101b (P0) agtmux-tmux-v5: capture + inspection + conversion + generation
  - Evidence: capture_pane, inspect_pane_processes, PaneGenerationTracker (5 tests), to_pane_snapshot (3 tests). cargo test -p agtmux-tmux-v5 PASS.
- [x] T-101a (P0) agtmux-tmux-v5: executor + list_panes parser
  - Evidence: TmuxCommandRunner trait, TmuxExecutor, tab-delimited list_panes parser (10 tests), TmuxPaneInfo, TmuxError (thiserror). cargo test -p agtmux-tmux-v5 PASS.
- [x] T-100a (P0) cursor contract fix: sources always return current position
  - Evidence: 3 sources fixed to always return `Some(current_pos)`. Gateway always overwrites tracker cursor. 2 new no-re-delivery tests added. 471 tests pass.
- [x] T-100 (P0) docs: runtime integration design
  - Evidence: 20_spec.md, 30_architecture.md (C-015/C-016 + MVP topology), 40_design.md (Section 9), 50_plan.md, 60_tasks.md, 90_index.md updated. ADR-20260225-mvp-single-process-runtime.md created. Codex + Opus review adopted.
- [x] T-033 (P2) poller baseline quality spec
  - Evidence: `docs/poller-baseline-spec.md` + `accuracy.rs` 12 tests + fixture 320 windows + `just poller-gate` PASS
- [x] T-041 (P2/P3) cursor contract hardening
  - Evidence: 18 tests pass, two-watermark + safe rewind + invalid cursor streak/resync
- [x] T-043 (P3) latency window SLO gate
  - Evidence: 15 tests pass, rolling p95 + breach counting + degraded alert
- [x] T-047 (P2/P3) UDS trust admission guard
  - Evidence: 15 tests pass, peer uid + source registry + nonce check
- [x] T-048 (P2/P3) source.hello + registry lifecycle
  - Evidence: 18 tests pass, 4-state lifecycle + hello handshake + staleness + socket rotation
- [x] T-049 (P3) snapshot/restore 基盤
  - Evidence: 15 tests pass, snapshot manager + policy + restore dry-run checker
- [x] T-051 (P4) observability alert routing
  - Evidence: 16 tests pass, severity-leveled alert ledger + auto-resolve + policy enforcement
- [x] T-052 (P4) supervisor strict runtime contract
  - Evidence: 18 tests pass, DependencyGate + FailureBudget + HoldDownTimer
- [x] T-053 (P3) binding projection 並行更新制御
  - Evidence: 15 tests pass, single-writer + CAS + conflict retry + rollback prevention
- [x] T-070 (P5) migration/canary/rollback runbook
  - Evidence: `docs/runbooks/migration-canary-rollback.md` + RP-T070
- [x] T-071 (P5) backup/restore runbook
  - Evidence: `docs/runbooks/backup-restore.md` + RP-T071
- [x] T-010 (P0) v5 crate/workspace skeleton
  - Evidence: 6 crates, `just verify` pass
- [x] T-020 (P1) tier resolver + unit/replay
  - Evidence: 35 resolver tests pass, dedup/freshness/rank suppression/re-promotion
- [x] T-011 (P1) poller logic reusable crate
  - Evidence: detect + evidence modules, 24 tests pass
- [x] T-012 (P1) source health FSM
  - Evidence: 31 health transition tests pass, 6-state FSM
- [x] T-013 (P1) title resolver + handshake priority
  - Evidence: 25 title tests pass, 5-tier priority + canonical session
- [x] T-030 (P2) codex appserver source server
  - Evidence: 10 tests pass, translate + source + cursor + health
- [x] T-031 (P2) claude hooks source server
  - Evidence: 11 tests pass, translate + source + cursor clamp fix
- [x] T-032 (P2) poller fallback server
  - Evidence: 40 tests pass, detection + evidence + pagination
- [x] T-040 (P2) gateway aggregation/cursor/health
  - Evidence: 23 tests pass, multi-source merge + cursor + health tracking
- [x] T-044 (P1/P3) pane signature classifier v1
  - Evidence: 27 tests pass, deterministic/heuristic/none + weights + guardrails
- [x] T-045 (P3) signature hysteresis/no-agent demotion
  - Evidence: 25 tests pass, idle/running/demotion windows + flap suppression
- [x] T-042 (P3) pane-first binding state machine
  - Evidence: 34 tests pass, 4-state FSM + generation tracking + tombstone grace + representative selection
- [x] T-050 (P3) daemon v5 projection + client API
  - Evidence: 25 tests pass, list_panes/list_sessions/changes_since + resolver integration
- [x] T-046 (P3) signature fields API exposure
  - Evidence: 9 new tests (34 total daemon), classifier integration + API contract + snapshot tests
- [x] T-060 (P4) supervisor + UI semantics
  - Evidence: 19 tests pass, restart backoff/holddown + startup order + UI labels (agents/unmanaged)
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
