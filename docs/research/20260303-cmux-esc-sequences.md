# Research: cmux State Detection & Claude Code Terminal Sequences

**Date**: 2026-03-03
**Trigger**: User question — how does `github.com/manaflow-ai/cmux` correctly detect
  `waiting_input`/`waiting_approval`, and are ESC sequences involved?
**Status**: Complete

---

## Executive Summary

**cmux does NOT use ESC/terminal sequences for state detection.** It uses Claude Code's
hook system exclusively. The key mechanism is the **`Notification` hook with
`notification_type` matcher** (`idle_prompt` / `permission_prompt`) — a signal that
agtmux's current implementation does not yet parse.

A supplementary finding: Claude Code sets the **terminal title** (via OSC 2) with a
braille spinner while running, which is readable as `#{pane_title}` in tmux and used by
the Rust project `tmuxcc` for heuristic state detection without hooks.

---

## Part 1: cmux Architecture

### Language & Runtime

- Native macOS app written in **Swift/AppKit** on top of **Ghostty's libghostty** terminal engine
- CLI tool (`CLI/cmux.swift`) is pure Swift, communicates via **Unix domain socket** (`/tmp/cmux.sock`)
- No ESC sequence parsing in state detection logic

### Detection Mechanism: Claude Code Hooks

cmux uses Claude Code's **hook system** for all state transitions. Two hook configurations:

#### Simple: Notification hook with matchers

```json
{
  "hooks": {
    "Notification": [
      {
        "matcher": "idle_prompt",
        "hooks": [{
          "type": "command",
          "command": "cmux notify --title 'Claude Code' --body 'Waiting for input'"
        }]
      },
      {
        "matcher": "permission_prompt",
        "hooks": [{
          "type": "command",
          "command": "cmux notify --title 'Claude Code' --subtitle 'Permission' --body 'Approval needed'"
        }]
      }
    ]
  }
}
```

The `matcher` field is matched against the `notification_type` field in the Claude hook JSON payload.

#### Advanced: `cmux claude-hook` subcommand

Used for sidebar "Running"/"Needs input" status pills:

| Event | Alias | Sidebar action |
|-------|-------|---------------|
| `session-start` | `active` | Sets "Running" (blue bolt icon) |
| `stop` | `idle` | Clears status; reads JSONL for last assistant message |
| `notification` | `notify` | Sets "Needs input" (blue bell icon) |

Typical `~/.claude/settings.json` for full integration:

```json
{
  "hooks": {
    "SessionStart": [{"type":"command","command":"cmux claude-hook session-start --session-id=$CLAUDE_SESSION_ID --workspace-id=$CMUX_WORKSPACE_ID --surface-id=$CMUX_PANEL_ID --cwd=$PWD"}],
    "Stop": [{"type":"command","command":"cmux claude-hook stop --session-id=$CLAUDE_SESSION_ID --transcript=$CLAUDE_TRANSCRIPT_PATH --message=\"$HOOK_LAST_ASSISTANT_MESSAGE\""}],
    "Notification": [{"type":"command","command":"cmux claude-hook notification --session-id=$CLAUDE_SESSION_ID --signal=$HOOK_NOTIFICATION_TYPE --message=\"$HOOK_NOTIFICATION_MESSAGE\""}]
  }
}
```

### Notification Classification Logic

Inside `classifyClaudeNotification()`, cmux does keyword matching on `signal + message`:

```swift
if lower.contains("permission") || lower.contains("approve") || lower.contains("approval") {
    return ("Permission", message)   // → waiting_approval
}
if lower.contains("idle") || lower.contains("wait") || lower.contains("input") || lower.contains("prompt") {
    return ("Waiting", message)      // → waiting_input
}
return ("Attention", message)
```

Signal (`notification_type`) is extracted from the hook JSON at these keys:
- `["event", "event_name", "hook_event_name", "type", "kind"]`
- `["notification_type", "matcher", "reason"]`
- Nested under `notification.type`, `data.kind`, etc.

### Claude `Notification` Hook Payload Structure

When Claude Code fires the `Notification` hook, the JSON body includes:

```json
{
  "notification_type": "idle_prompt",   // or "permission_prompt"
  "message": "Claude is waiting for your input",
  "session_id": "...",
  "transcript_path": "/path/to/transcript.jsonl"
}
```

Known `notification_type` values:
- `"idle_prompt"` — Claude finished responding, awaiting user input
- `"permission_prompt"` — Permission approval dialog is showing

---

## Part 2: Claude Code Terminal Sequences (Complete Inventory)

### OSC 2 / OSC 0 — Terminal Title (state-bearing ✓)

```
\033]2;<spinner> Claude Code\007   # while running
\033]2;Claude Code\007             # while idle
\033]2;Claude: <session-name>\007  # after /rename
```

**State encoding via braille spinner:**
- Running/Processing: title has a cycling braille character prefix
- Spinner set: `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` (introduced in v2.1.6, jitter-fixed in v2.1.7)
- Idle/Waiting: title is bare `Claude Code` (no spinner prefix)

**tmux access:** `tmux display-message -t <target> -p '#{pane_title}'`

Used by [tmuxcc](https://github.com/nyanko3141592/tmuxcc): check if `pane.title.chars().any(|c| matches!(c, '⠿'|'⠇'|'⠋'|'⠙'|'⠸'|'⠴'|'⠦'|'⠧'|'⠖'|'⠏'|'⠹'|'⠼'|'⠷'|'⠾'|'⠽'|'⠻'|'⠐'|'⠑'|'⠒'|'⠓'))`.

### OSC 9;4 — Windows Progress Bar (partial, terminal-dependent)

```
\033]9;4;3;0\007   # state=3 (indeterminate) while running
\033]9;4;0;0\007   # state=0 (clear) when done
```

States:

| Value | Meaning |
|-------|---------|
| `0` | Clear/remove progress |
| `1` | Normal (green), requires `percent` |
| `2` | Error (red) |
| `3` | Indeterminate (pulsing) |
| `4` | Paused/warning (orange) |

**Critical negative:** OSC 9;4 remains at state `3` (indeterminate) even during
`waiting_input`. It **cannot** distinguish running from waiting. (Issue #12620 confirms
the animation continues during user input prompts.)

Supported terminals: Windows Terminal, Ghostty, iTerm2 (partial). Not in Terminal.app.

### OSC 9 — Desktop Notifications (one-shot)

```
\033]9;<message>\007
# Inside tmux (DCS passthrough):
\033Ptmux;\033\033]9;<message>\007\033\\
```

One-shot push notification. Not state-bearing for running/waiting purposes.

### OSC 8 — Clickable Hyperlinks (not state-bearing)

Used for file paths in tool output. No state information.

### OSC 10/11 — Color Queries (queries, not state)

Claude Code **queries** `\033]10;?\007` and `\033]11;?\007` to detect terminal
background/foreground colors. Responses can leak into input buffer (Issue #12910).

### No APC / DCS / OSC 133

- **APC** (`\033_...\033\\`): Not emitted by Claude Code
- **DCS** (`\033P...\033\\`): Only appears as tmux passthrough wrapper in hook scripts
- **OSC 133**: Confirmed not emitted (shell integration; ADR-20260301-osc-architecture.md)

---

## Part 3: Permission Approval Dialog — No Dedicated Sequence

**Critical negative finding:** The permission approval dialog ("Yes / Yes, and don't ask
again / No") emits **no dedicated ESC sequence**. No OSC code changes during the dialog.

Detection methods used in the community:
- **PermissionRequest hook** (best): out-of-band, no parsing needed ← agtmux already uses this
- **Text pattern** (heuristic): scan `capture-pane` output for button lines

tmuxcc's `detect_yes_no_buttons()` pattern (from `claude_code.rs`):
1. Take last 8 lines of `capture-pane`
2. Look for short lines matching "Yes" / "Yes, and don't ask again for this session" / "No"
3. Check that two such lines are within 4 lines of each other

---

## Part 4: Implications for agtmux

### Gap Found: `Notification` Hook Not Parsed

agtmux registers the `Notification` hook in `setup_hooks.rs` HOOK_TYPES (11 total), but
`normalize_event_type()` in `crates/agtmux-source-claude-hooks/src/translate.rs` has no
arm for `"Notification"` — it falls through to `"lifecycle.unknown"`.

When Claude finishes a turn, the hook fire order is:
1. `Stop` → `activity.waiting_input` ← **already handled** (T-codex03)
2. `Notification` with `notification_type = "idle_prompt"` → `lifecycle.unknown` ← **gap**

For permission dialogs:
1. `PermissionRequest` → `activity.waiting_approval` ← **already handled** (T-E01)
2. `Notification` with `notification_type = "permission_prompt"` → `lifecycle.unknown` ← **gap**

The `Notification` hook with `notification_type` is a **stronger, more specific signal**
than `Stop` because it is explicitly typed. However, since `Stop` and `PermissionRequest`
already work correctly, the `Notification` gap is lower urgency (belt-and-suspenders).

**Recommended fix (new task T-E05):**

In `translate()`, when `hook_type == "Notification"`, read `raw.data["notification_type"]`:

```rust
"Notification" => {
    match raw.data.get("notification_type").and_then(|v| v.as_str()) {
        Some("idle_prompt") => "activity.waiting_input".to_owned(),
        Some("permission_prompt") => "activity.waiting_approval".to_owned(),
        _ => "lifecycle.notification".to_owned(),
    }
}
```

This requires changing `normalize_event_type(hook_type: &str)` to also take `data: &serde_json::Value`.

### Enhancement Opportunity: Pane Title Spinner Detection

The braille spinner in `pane_title` is a viable supplementary heuristic signal for
`running` state detection **without hooks**. Useful for panes where hooks are not
configured.

Implementation approach:
- In the tmux poller (`agtmux-source-poller` or a new `agtmux-source-pane-title`):
  - Read `pane_title` via `tmux display-message -t <pane_id> -p '#{pane_title}'`
  - Check for braille chars (range U+2800–U+283F, specifically `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ⠿⠇⠸⠴⠦⠧⠖⠏⠹⠼⠷⠾⠽⠻⠐⠑⠒⠓`)
  - Present → `activity.running` (heuristic, tier ≥ 2)
  - Absent + title contains "Claude Code" → `activity.idle` (heuristic)
- Confidence: ~0.85 (heuristic, not deterministic)
- Does NOT distinguish `waiting_input` from `idle` (spinner is absent for both)
- Does NOT detect `waiting_approval` (spinner may still be present during dialog)

Classification: **Post-MVP supplement** for hooks-free environments.

### Summary Table

| Detection method | Source | waiting_input | waiting_approval | running | Notes |
|-----------------|--------|:---:|:---:|:---:|-------|
| `Stop` hook | claude-hooks | ✅ | — | — | Already implemented |
| `PermissionRequest` hook | claude-hooks | — | ✅ | — | Already implemented |
| `Notification` hook (idle_prompt) | claude-hooks | ⚠️ gap | — | — | T-E05 needed |
| `Notification` hook (permission_prompt) | claude-hooks | — | ⚠️ gap | — | T-E05 needed |
| `PreToolUse` / `PostToolUse` hooks | claude-hooks | — | — | ✅ | Already implemented |
| Braille spinner in `pane_title` | pane-title poller | ❌ no | ❌ no | ✅ heuristic | Post-MVP |
| OSC 9;4 progress bar | osc-tap | ❌ no | ❌ no | ⚠️ partial | Too terminal-dependent |
| Terminal content text patterns | capture-pane | ⚠️ fragile | ⚠️ fragile | ⚠️ fragile | Fallback only |

---

## References

- [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) — Swift/Ghostty terminal for AI agents
- [nyanko3141592/tmuxcc](https://github.com/nyanko3141592/tmuxcc) — Rust TUI for AI coding agents
- [claude-code issue #17887](https://github.com/anthropics/claude-code/issues/17887) — Braille spinner tab-width jitter
- [claude-code issue #23793](https://github.com/anthropics/claude-code/issues/23793) — Terminal title customization request
- [claude-code issue #12620](https://github.com/anthropics/claude-code/issues/12620) — OSC 9;4 stays indeterminate during waiting
- [claude-code issue #12910](https://github.com/anthropics/claude-code/issues/12910) — OSC 10/11 color query response leaks
- [OSC 9;4 spec](https://rockorager.dev/misc/osc-9-4-progress-bars/)
- `docs/80_decisions/ADR-20260301-osc-architecture.md` — OSC 133 negative finding
- `crates/agtmux-source-claude-hooks/src/translate.rs` — current hook mappings
