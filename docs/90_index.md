# Codebase Index (generated; can be stale)

## Start Here (MVP)
1. `docs/00_router.md` (`Execution Mode B` を確認)
2. `docs/60_tasks.md` (`MVP Track` の先頭から着手)
3. `docs/20_spec.md` (`[MVP]` 要件のみ読む)
4. `docs/40_design.md` (`Main (MVP Slice)` のみ読む)
5. `docs/50_plan.md` (Phase 1-2)

## Hardening Later (Post-MVP)
- `docs/20_spec.md` の `[Post-MVP]` FR
- `docs/40_design.md` の `Appendix (Post-MVP Hardening)`
- `docs/60_tasks.md` の `Post-MVP Backlog`

## Entry points
- Docs router: `docs/00_router.md`
- Foundation/spec: `docs/10_foundation.md`, `docs/20_spec.md`
- Architecture/design/plan: `docs/30_architecture.md`, `docs/40_design.md`, `docs/50_plan.md`
- Execution board: `docs/60_tasks.md`, `docs/70_progress.md`
- Local command entrypoint: `justfile`

## Key directories
- `docs/`
  - v5 blueprint docs (`00`〜`90`)
- `providers/`
  - provider TOML definitions（v4互換資産）
- Reference implementation (v4):
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-core`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-daemon`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-tmux`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/fixtures`

## Where to find X
- MVP requirement boundary:
  - `docs/20_spec.md` (`[MVP]` / `[Post-MVP]` tags)
  - `docs/80_decisions/ADR-20260225-core-first-mode-b.md`
- Tiered resolver policy:
  - `docs/20_spec.md` (FR-001〜FR-006)
  - `docs/40_design.md` (Main -> Resolver and Arbitration)
- Managed/unmanaged + deterministic/heuristic 命名:
  - `docs/20_spec.md` (Terminology)
- Pane signature v1:
  - `docs/20_spec.md` (FR-024〜FR-031)
  - `docs/40_design.md` (Main -> Pane Signature Classifier)
  - `docs/80_decisions/ADR-20260225-pane-signature-v1.md`
- Poller fallback受入基準:
  - `docs/20_spec.md` (FR-032〜FR-033)
  - `docs/40_design.md` (Main -> Test Strategy)
- Post-MVP hardening (ack/registry/supervisor/snapshot):
  - `docs/20_spec.md` (FR-018〜FR-020, FR-034〜FR-047)
  - `docs/40_design.md` (Appendix)

## Where to find X (continued)
- Runtime integration (MVP single-process):
  - `docs/40_design.md` (Main -> 9) Runtime Integration)
  - `docs/30_architecture.md` (C-015, C-016, Runtime Topology MVP)
  - `docs/80_decisions/ADR-20260225-mvp-single-process-runtime.md`

## Decisions
- `docs/80_decisions/ADR-2026-02-25-v5-mvp-source-policy.md`
- `docs/80_decisions/ADR-20260225-cursor-binding-latency.md`
- `docs/80_decisions/ADR-20260225-pane-signature-v1.md`
- `docs/80_decisions/ADR-20260225-operational-guards.md`
- `docs/80_decisions/ADR-20260225-runtime-control-contracts.md`
- `docs/80_decisions/ADR-20260225-core-first-mode-b.md`
- `docs/80_decisions/ADR-20260225-mvp-single-process-runtime.md`

## How to run (local-first)
- quality gates:
  - `just verify`
- individual checks:
  - `just fmt`
  - `just lint`
  - `just test`
- online/e2e source tests:
  - `just preflight-online`
  - `just test-source-codex`
  - `just test-source-claude`
  - `just test-source-poller`
