# Review Pack — T-codex03: waiting_input/waiting_approval Detection Fixes

## Objective
- Task: T-codex03
- Acceptance criteria: `waiting_input`/`waiting_approval` activity states are correctly
  emitted and exposed in `agtmux json` output so that agtmux-term notification badges work.

## Summary
Three bugs were causing non-idle/running states to collapse into `idle` or `unknown`:

1. **Codex WaitingInput → `activity.idle` (wrong)**: `translate.rs` mapped
   `CodexSessionState::WaitingInput` to `"activity.idle"` instead of
   `"activity.waiting_input"`. After `task_complete`/`turn_aborted`, panes showed `idle`
   instead of `waiting_input`.

2. **Claude Stop/SubagentStop → `lifecycle.unknown` (wrong)**: `normalize_event_type()` in
   claude-hooks `translate.rs` had no mapping for `Stop`/`SubagentStop`, so they fell to
   `"lifecycle.unknown"` → `ActivityState::Unknown` (precedence 0), which never beat
   `Running` (precedence 2). Stop hook was silently ignored.

3. **No e2e coverage for Claude PermissionRequest/Stop**: Added `test-claude-approval.sh`
   to contract test suite, completing the 4-phase test: tool_start→running,
   PermissionRequest→waiting_approval, recovery→running, Stop→waiting_input.

Research confirmed Claude JSONL transcript has no line types for waiting states; hooks
are the only mechanism. `docs/research/claude-jsonl-waiting-states.md` documents this.

## Change scope

| File | Change |
|------|--------|
| `crates/agtmux-source-codex-jsonl/src/translate.rs` | Fix WaitingInput→waiting_input + unit test rename |
| `crates/agtmux-source-codex-jsonl/src/source.rs` | Unit test rename + assertion update |
| `crates/agtmux-source-claude-hooks/src/translate.rs` | Add Stop/SubagentStop mapping + unit test cases |
| `scripts/tests/e2e/contract/test-claude-approval.sh` | NEW: 4-phase contract test |
| `scripts/tests/e2e/contract/run-all.sh` | Add test-claude-approval.sh (11 total) |
| `scripts/tests/e2e/scenarios/codex-semantic-states.sh` | idle → waiting_input assertion |
| `scripts/tests/e2e/scenarios/codex-tool-execution.sh` | idle → waiting_input assertion |
| `scripts/tests/e2e/scenarios/codex-approval-flow.sh` | idle → waiting_input assertion |
| `scripts/tests/e2e/scenarios/codex-session-rotation.sh` | idle → waiting_input (2 places) |
| `scripts/tests/e2e/scenarios/codex-title.sh` | idle → waiting_input assertion |
| `docs/research/claude-jsonl-waiting-states.md` | NEW: research findings |
| `docs/60_tasks.md` | T-codex03 task entry added |

No changes to `projection.rs` (already handles both states) or `test-schema.sh` (both
already listed as valid enum values).

## Verification evidence

- `cargo test -p agtmux-source-codex-jsonl` → **52 tests PASS**
- `cargo test -p agtmux-source-claude-hooks` → **14 tests PASS**
- `just verify` (fmt + clippy + all unit tests) → **PASS, 0 warnings**
- `bash scripts/tests/e2e/contract/run-all.sh` → **11/11 PASS** (including new test-claude-approval.sh)
- `bash scripts/tests/e2e/scenarios/codex-semantic-states.sh` → **PASS** (waiting_input ✓)
- `bash scripts/tests/e2e/scenarios/codex-tool-execution.sh` → **PASS** (waiting_input ✓)
- `bash scripts/tests/e2e/scenarios/codex-approval-flow.sh` → **PASS** (waiting_input ✓)
- `bash scripts/tests/e2e/scenarios/codex-session-rotation.sh` → **PASS** (waiting_input ✓)

Note: `codex-title.sh` is an online scenario (requires live Codex). Its `idle→waiting_input`
change at line 61 is consistent with the other 4 scenarios; no logic change, only the
expected state string updated.

## Risk declaration
- Breaking change: No — `activity.waiting_input` was the intended value; the old
  `activity.idle` was a bug. Consumers expecting `idle` after task_complete were already
  getting incorrect data.
- Fallbacks: None added (per policy: fail loudly).
- Known gaps / follow-ups: None. `codex-title.sh` online scenario not re-verified (requires
  live Codex run); safe to verify on next online test session.

## Reviewer request
Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
