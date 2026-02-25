# Router & Contract (always read first)

## L1: Non-negotiables (Hard Gates)
- `docs/` is the ONLY source of truth. If it is not in `docs/`, it is not authoritative.
- Router is process-only. Product intent/spec lives in `docs/10_foundation.md` and above, not in this file.
- Orchestrator MUST delegate Implement / Test / Review to separate subagents.
- Final Go/Stop decision is made ONLY by Orchestrator.
- Only Orchestrator may edit:
  - `docs/60_tasks.md`, `docs/70_progress.md`, `docs/80_decisions/*`, `docs/85_reviews/*`
- Local-first execution is default. Daily development/testing MUST NOT require commit/PR workflow.
- If commit/PR/release is performed, Quality Gates MUST pass first (see below).
- If unsure, be fail-closed: STOP or escalate (Escalation Matrix).

## L1.5: Execution Mode (B: Core-first)
- Current mode is `B` (core spec + implementation feedback).
- During Phase 1-2, only items tagged `[MVP]` in `docs/20_spec.md` are implementation blockers.
- Items tagged `[Post-MVP]` are valid design assets but must NOT block Phase 1-2 coding.
- If `[Post-MVP]` work is discovered as unexpectedly necessary, Orchestrator must:
  - create a task in `docs/60_tasks.md` with clear dependency and rationale
  - record the decision in `docs/70_progress.md`
  - escalate only when it changes `docs/10_foundation.md` or public behavior

## L2: Progressive Disclosure (What to read, in order)
1) `docs/70_progress.md` (latest learnings, constraints, open points)
2) `docs/60_tasks.md` (`MVP Track` first)
3) `docs/10_foundation.md` (stable intent)
4) `docs/20_spec.md` (`[MVP]` first, `[Post-MVP]` only as needed)
5) `docs/40_design.md` (`Main (MVP Slice)` first, `Appendix` only if blocked)
6) `docs/30_architecture.md` -> `docs/50_plan.md` (as needed)
7) `docs/90_index.md` (only if structure changed / cannot navigate)

## Plan mode policy (Docs-first)
- Built-in plan/task outputs are scratch.
- In plan mode, DO NOT create a separate plan document.
- Output ONLY: "Proposed edits to docs/*" (file-by-file patch suggestions) + "Proposed updates to docs/60_tasks.md".
- After approval, apply edits to `docs/*` BEFORE writing code.

### Plan mode output format (mandatory)
A) Proposed edits:
- File: `docs/20_spec.md`
  - Section: ...
  - Replace/Add: ...
- File: `docs/40_design.md`
  - ...

B) Proposed task board update:
- Add/modify tasks in `docs/60_tasks.md` (IDs stable; keep history)

C) Open questions ONLY if Escalation triggers.

## Quality Gates (project-specific)
- Format: `just fmt` must PASS (`cargo fmt --all -- --check`)
- Typecheck/Lint: `just lint` must PASS (strict clippy deny flags)
- Tests: `just test` must PASS (`cargo test --workspace --all-features --locked`)
- Unified local gate: `just verify` (fmt + lint + test) must PASS before review/commit/PR.
- Online/e2e source tests MUST run `just preflight-online` before `just test-source-codex` / `just test-source-claude`.
  - Preflight must fail-closed on tmux/CLI auth/network readiness.
- Task-specific gate suites in `docs/60_tasks.md` (contract/integration/regression) must PASS for touched scope.
- Reviewer verdict required: `GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO`

## Review protocol (prevent stall)
- Reviewer does NOT run tests (Tester does). Reviewer judges using Review Pack only.
- Orchestrator MUST create a Review Pack in `docs/85_reviews/` before requesting review.
- Verdict schema:
  - `GO`
  - `GO_WITH_CONDITIONS` (ship + create follow-up tasks)
  - `NO_GO` (must fix)
  - `NEED_INFO` (max 3 missing items; Orchestrator supplies and re-review)

### NEED_INFO loop
- If `NEED_INFO`: Orchestrator supplies ONLY the requested evidence and re-runs review.
- If `NEED_INFO` repeats twice:
  - switch reviewer (second reviewer), OR
  - proceed with `GO_WITH_CONDITIONS` + create explicit follow-up tasks, OR
  - escalate to user (if risk is high).

## Escalation Matrix (ask user)
- Change to `docs/10_foundation.md` (persona/user story/goals/non-goals/global AC)
- Breaking public API / CLI compatibility or major behavior change
- Auth/permissions, billing/payment, data deletion, migrations
- Change to freshness/down thresholds (`3s` / `15s`) or fallback policy
- Change to poller fallback quality baseline (currently ~85%)
- Large dependency bumps with wide blast radius
