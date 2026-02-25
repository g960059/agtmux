# Codebase Index (generated; can be stale)

## Entry points
- Docs router: `docs/00_router.md`
- Foundation/spec: `docs/10_foundation.md`, `docs/20_spec.md`
- Architecture/design/plan: `docs/30_architecture.md`, `docs/40_design.md`, `docs/50_plan.md`
- Execution board: `docs/60_tasks.md`, `docs/70_progress.md`
- Local command entrypoint: `justfile`

## Key directories
- `docs/`
  - v5 blueprint docs (`00`〜`90`)
  - `v3/` legacy design context
- `providers/`
  - provider TOML definitions（v4互換資産）
- Reference implementation (v4):
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-core`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-daemon`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/crates/agtmux-tmux`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/fixtures`

## Where to find X
- Tiered resolver policy:
  - `docs/20_spec.md` (requirements)
  - `docs/40_design.md` (algorithm)
- Managed/unmanaged と deterministic/heuristic の命名規約:
  - `docs/20_spec.md` (Terminology)
  - `docs/80_decisions/ADR-2026-02-25-v5-mvp-source-policy.md`
- Source priority/fallback:
  - `docs/30_architecture.md` (component/data flow)
  - `docs/40_design.md` (admissibility)
- Runtime orchestration:
  - `docs/30_architecture.md` (`agtmux-runtime-supervisor`)
- Tasks/progress:
  - `docs/60_tasks.md`, `docs/70_progress.md`
- Decisions:
  - `docs/80_decisions/`
  - `docs/80_decisions/ADR-2026-02-25-v5-mvp-source-policy.md`
- Reviews:
  - `docs/85_reviews/`

## How to run (local-first)
- dev daemon:
  - `cargo run -p agtmux-daemon -- daemon`
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
