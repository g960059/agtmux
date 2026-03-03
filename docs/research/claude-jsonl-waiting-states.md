# Research: Claude JSONL Waiting States

**Date**: 2026-03-03
**Status**: Complete — No new JSONL mappings needed
**Conclusion**: Waiting states require hooks only; JSONL transcript cannot signal them.

---

## Summary

Claude Code JSONL transcript files do **not** contain line types representing
`waiting_approval` or `waiting_input` states. These signals are only available
via the hooks system (`PermissionRequest`, `Stop`, `SubagentStop`).

No changes to `agtmux-source-claude-jsonl/src/translate.rs` are needed for
waiting state detection.

---

## Current JSONL Line Types in agtmux

File: `crates/agtmux-source-claude-jsonl/src/translate.rs`

| Line type | Mapped event | Notes |
|-----------|-------------|-------|
| `"user"` | `activity.user_input` | User message |
| `"tool_use"` | `activity.running` | Claude calling a tool |
| `"tool_result"` | `activity.tool_complete` | Tool response returned |
| `"assistant"` | `activity.idle` | Assistant text response |
| `"progress"` | `activity.running` | ~1/sec during tool execution |
| `"custom-title"` | (title extraction) | T-135b: session rename via `/rename` |
| `"summary"` | (summary extraction) | T-135c: AI-generated summary |
| `"system"` | (skipped) | System prompt lines |
| `"file-history-snapshot"` | (skipped) | File state snapshot |
| `"queue-operation"` | (skipped) | Internal queue events |

---

## Findings: No JSONL Signals for Waiting States

### `waiting_approval` (PermissionRequest)

- Triggered when Claude Code displays a permission approval dialog.
- The `PermissionRequest` hook fires at this moment — already mapped in
  `agtmux-source-claude-hooks/src/translate.rs`:
  `"PermissionRequest" → "activity.waiting_approval"`
- **No corresponding JSONL line type exists.** Permission requests are not
  written to the transcript file; they are only observable via hooks.

### `waiting_input` (Stop / SubagentStop)

- Triggered when Claude pauses and awaits the next user message.
- The `Stop` and `SubagentStop` hooks fire at this moment — mapped (as of
  this task) in `agtmux-source-claude-hooks/src/translate.rs`:
  `"Stop" | "SubagentStop" → "activity.waiting_input"`
- **No corresponding JSONL line type exists.** The transcript does not record
  a "waiting for input" sentinel line.
- The `stop_reason` field in JSONL assistant messages (`"end_turn"`,
  `"max_tokens"`, `"tool_use"`) describes why Claude stopped generating text,
  not that the session is now waiting for user input. These are not useful
  signals for activity state.

---

## Architecture Implication

```
agtmux-source-claude-hooks  (rank 0, deterministic)
  ← PermissionRequest → waiting_approval  ✓
  ← Stop / SubagentStop → waiting_input   ✓ (fixed in this task)

agtmux-source-claude-jsonl  (rank 1, deterministic)
  ← tool_use/progress → running           ✓
  ← assistant → idle                      ✓
  ← waiting_approval / waiting_input      ✗ (no JSONL line types available)
```

JSONL provides deterministic evidence for Running/Idle when hooks are
unavailable (e.g., hooks not registered), but it cannot replicate
waiting-state detection. This asymmetry is intentional and acceptable:
users who have hooks registered get full state visibility; users without
hooks get Running/Idle only.

---

## Recommendation

1. **No new JSONL mappings needed** — no line types to map.
2. **Hooks remain the only mechanism** for `waiting_input` and `waiting_approval`.
3. **Enhancement opportunity (Post-MVP)**: If Claude Code adds JSONL line
   types for permission requests or stop-waiting states in a future release,
   add corresponding mappings to `translate.rs` following the same pattern
   used for `custom-title` and `summary`.

---

## References

- [Claude Code hooks reference](https://code.claude.com/docs/en/hooks)
- [Claude API stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)
- GitHub issue #13024: Feature Request — hook when Claude is waiting for user input
- GitHub issue #29212: PermissionRequest hook fires for every tool check
- `crates/agtmux-source-claude-hooks/src/translate.rs` — current hook mappings
- `crates/agtmux-source-claude-jsonl/src/translate.rs` — current JSONL mappings
