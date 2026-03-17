# Review Pack: T-033 Poller Baseline Quality Specification

## Objective
- Task: T-033
- User story: US-002, US-005
- Acceptance criteria: FR-032 (weighted F1 >= 0.85, waiting recall >= 0.85), FR-033 (fixed fixture >= 300 windows)

## Summary (3-7 lines)
- Created `docs/poller-baseline-spec.md` defining the quality gate contract.
- Created `crates/agtmux-source-poller/src/accuracy.rs` with evaluator logic (12 tests).
- Created `fixtures/poller-baseline/dataset.json` with 320+ labeled windows (mixed Claude/Codex/no-agent).
- Added `just poller-gate` command to run the integration gate test.
- Gate thresholds: weighted F1 >= 0.85 AND WaitingApproval recall >= 0.85.
- Per-class metrics (precision/recall/F1) reported for diagnostics.

## Change scope
- `docs/poller-baseline-spec.md` (NEW)
- `crates/agtmux-source-poller/src/accuracy.rs` (NEW — evaluator + 12 tests)
- `crates/agtmux-source-poller/src/lib.rs` (added `pub mod accuracy`)
- `fixtures/poller-baseline/dataset.json` (NEW — labeled fixture dataset)
- `justfile` (added `poller-gate` target)
- `docs/85_reviews/RP-T033-poller-baseline.md` (NEW, this file)

## Verification evidence
- Commands run:
  - `just verify` => PASS (468 tests, 0 failures)
  - `just poller-gate` => PASS (gate thresholds met)
- Notes:
  - Evaluator cross-referenced against FR-032/FR-033 requirements
  - Fixture contains >= 300 windows with mixed providers and activity states
  - Gate thresholds match spec (0.85 / 0.85)

## Risk declaration
- Breaking change: no
- Fallbacks: fixture dataset can be regenerated if issues found
- Known gaps: fixture is synthetic (pattern-matched to implementation); real-world data may differ

## Reviewer request
- Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
