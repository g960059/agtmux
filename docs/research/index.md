# Research Index

All research documents in `docs/research/`. Newest first.

---

## 2026-03-03

### [cmux State Detection & Claude Code Terminal Sequences](./20260303-cmux-esc-sequences.md)

**Trigger**: How does `github.com/manaflow-ai/cmux` detect `waiting_input`/`waiting_approval`?
Are ESC sequences involved?

**Key findings:**
- cmux uses Claude Code hooks exclusively; **no ESC sequences** for state detection
- The `Notification` hook with `notification_type` matcher (`idle_prompt` / `permission_prompt`)
  is the primary cmux mechanism — distinct from the `Stop`/`PermissionRequest` hooks
- **agtmux gap discovered**: `Notification` hook falls through to `lifecycle.unknown`
  (new task T-E05 recommended)
- Claude Code **braille spinner in `pane_title`** (OSC 2) is a viable heuristic for
  detecting `running` without hooks (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`)
- OSC 9;4 progress bar stays at indeterminate during `waiting_input` — cannot
  distinguish running from waiting
- Permission dialog has **no dedicated ESC sequence** — hooks or text patterns only

**Actionable tasks identified:**
- T-E05 (P2): Handle `Notification` hook `notification_type` in `translate.rs`
- Post-MVP: Pane-title braille spinner source for hooks-free running detection

---

### [Claude JSONL Waiting States](./claude-jsonl-waiting-states.md)

**Trigger**: Can Claude JSONL transcripts signal `waiting_input` / `waiting_approval`?

**Key findings:**
- Claude JSONL has **no line types** for waiting states
- Hooks are the only mechanism for these signals
- No changes to `agtmux-source-claude-jsonl` needed

---

## 2026-03-02

### [Codex JSONL Source Design Research](./20260302/)

4-agent parallel research session investigating the root cause of Codex state
detection failures and designing the replacement JSONL FSM architecture.

| File | Contents |
|------|----------|
| [00_overview.md](./20260302/00_overview.md) | Problem statement |
| [01_bug_investigation.md](./20260302/01_bug_investigation.md) | Root cause analysis |
| [02_proposals_comparison.md](./20260302/02_proposals_comparison.md) | 4 design proposals compared |
| [03_unified_design_v0.md](./20260302/03_unified_design_v0.md) | Unified design draft |
| [04_research_questions.md](./20260302/04_research_questions.md) | Open questions |
| [05_synthesis.md](./20260302/05_synthesis.md) | Final synthesis — basis for Phase 9 implementation |

**Key findings:**
- JSONL JSON key is `.payload.type` (not `.data.type`)
- Correct event names: `task_started`, `task_complete`, `entered_review_mode`, `exited_review_mode`
- No keepalive writes — Idle = file write stops
- `entered_review_mode` is the definitive WaitingApproval signal
- Result: Phase 9 FSM source (`agtmux-source-codex-jsonl`) implementation
