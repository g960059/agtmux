# Review Pack — T-E05 / T-E06 / T-E07: Hooks & Title State Detection

## Objective
- Tasks: T-E05, T-E06, T-E07
- Acceptance criteria:
  - T-E05: `Notification` hook with `notification_type = idle_prompt/permission_prompt` emits correct activity states
  - T-E06: `PreToolUse`/`PostToolUse` hooks emit `activity.running`
  - T-E07: Braille spinner in `pane_title` promotes Unknown/Idle → Running (Claude-only, heuristic)

## Summary

Three incremental fixes to hook translation and the poller source, based on cmux research findings:

### T-E05 — Notification hook notification_type parsing

**File:** `crates/agtmux-source-claude-hooks/src/translate.rs`

Added `resolve_event_type(hook_type: &str, data: &serde_json::Value) -> String` wrapper function.
When `hook_type == "Notification"`, reads `data["notification_type"]`:
- `"idle_prompt"` → `"activity.waiting_input"`
- `"permission_prompt"` → `"activity.waiting_approval"`
- unknown/absent → `"lifecycle.notification"`

`normalize_event_type()` signature unchanged (pure `&str → String`).
Call site in `translate()` changed from `normalize_event_type` to `resolve_event_type`.

### T-E06 — PreToolUse/PostToolUse → activity.running

**File:** same `translate.rs`

Added arm to `normalize_event_type()`:
```rust
"PreToolUse" | "PostToolUse" => "activity.running".to_owned(),
```

### T-E07 — Braille spinner pane_title detection

**Files:**
- `crates/agtmux-source-poller/src/evidence.rs`: added 6 missing braille chars (`⠼⠴⠦⠧⠇⠏`) to `claude_activity_signals()` Running patterns. Full set now `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`.
- `crates/agtmux-source-poller/src/source.rs`: added `classify_claude_title_activity(title: &str) -> Option<ActivityState>` and title-upgrade logic in `poll_pane()`.

Title upgrade guard: Claude-only, Unknown/Idle current state only, spinner-absent is no-op.

## Change scope

| File | Change |
|------|--------|
| `crates/agtmux-source-claude-hooks/src/translate.rs` | Add `resolve_event_type()` + T-E06 arms + 6 unit tests |
| `crates/agtmux-source-poller/src/evidence.rs` | Add 6 missing braille chars to Running patterns |
| `crates/agtmux-source-poller/src/source.rs` | Add `classify_claude_title_activity()` + upgrade logic + 8 unit tests |
| `docs/60_tasks.md` | T-E06, T-E07 task entries added |

## Verification evidence

- `just verify` (fmt + clippy + all unit tests) → **PASS, 0 warnings**
- `cargo test -p agtmux-source-claude-hooks` → **20 tests PASS** (6 new)
- `cargo test -p agtmux-source-poller` → **77 tests PASS** (8 new)
- Total workspace: 19 test binaries, all PASS

### T-E07 test coverage

| Test | Verifies |
|------|---------|
| `title_with_spinner_returns_running` | spinner → Some(Running) |
| `title_without_spinner_returns_none` | no spinner → None |
| `empty_title_returns_none` | empty → None |
| `title_with_each_spinner_char_returns_running` | all 10 chars individually |
| `poll_pane_spinner_title_upgrades_unknown_to_running` | Unknown → Running via title |
| `poll_pane_spinner_title_upgrades_idle_to_running` | Idle → Running via title |
| `poll_pane_no_spinner_title_does_not_override_running` | absence is no-op |
| `poll_pane_spinner_title_does_not_override_waiting_approval` | WaitingApproval not overridden |
| `poll_pane_spinner_title_codex_pane_no_upgrade` | Codex pane not upgraded |

## Risk declaration

- **T-E05 breaking change**: No. `Notification` previously fell to `lifecycle.unknown`; `activity.waiting_input`/`waiting_approval` are stronger signals that improve correctness. No consumer expected `lifecycle.unknown` from Notification.
- **T-E06 breaking change**: No. `PreToolUse`/`PostToolUse` previously fell to `lifecycle.unknown`; `activity.running` is unambiguously correct.
- **T-E07 breaking change**: None. Heuristic-only, only promotes from Unknown/Idle, never overrides deterministic states. Opt-out not needed since it's a net improvement.
- **Fallbacks**: None added (per policy).
- **Known gaps / follow-ups**: T-E07 braille chars are matched against `capture_lines` in evidence.rs for capture-based detection; the title-based detection uses the full set via the new `classify_claude_title_activity` function.

## Reviewer request

Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
