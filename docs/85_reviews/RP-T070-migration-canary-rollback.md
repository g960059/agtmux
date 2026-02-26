# Review Pack: T-070 Migration / Canary / Rollback Runbook

## Objective
- Task: T-070
- User story: US-005
- Acceptance criteria: FR-038 (snapshot/restore), FR-039 (supervisor startup), FR-040 (failure budget)

## Summary (3-7 lines)
- Created `docs/runbooks/migration-canary-rollback.md` covering the full v4→v5 migration lifecycle.
- 4-phase rollout: Internal Alpha → Canary → Gradual Default → Full Cutover.
- Each phase has explicit entry/exit criteria with operator sign-off checkpoints.
- Instant rollback via runtime selector at any phase, with clear trigger signals.
- References supervisor contract parameters (backoff, budget, hold-down) from T-060/T-052 implementations.
- Canary gate checklist provided as final sign-off before cutover.

## Change scope
- `docs/runbooks/migration-canary-rollback.md` (NEW)
- `docs/85_reviews/RP-T070-migration-canary-rollback.md` (NEW, this file)

## Verification evidence
- Runbook cross-referenced against:
  - `docs/50_plan.md` rollout/rollback sections
  - `docs/30_architecture.md` Flow-010, Flow-011
  - `docs/40_design.md` A4 (supervisor), A7 (snapshot)
  - ADR-20260225-runtime-control-contracts.md
- Supervisor parameters match `RestartPolicy::default()` values
- Snapshot dry-run procedure matches `RestoreDryRun` API

## Risk declaration
- Breaking change: no (documentation only)
- Fallbacks: v4 daemon path available at all phases
- Known gaps: Actual runtime selector configuration format TBD (implementation-dependent)

## Reviewer request
- Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
