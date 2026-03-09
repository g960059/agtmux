# Research Index

Current, compact entry points for `docs/research/`.

## Current

- [20260309-status-notification-comparison.md](./20260309-status-notification-comparison.md)
  - Cross-tool comparison of `cmux`, `CodexMonitor`, and OpenAI `codex`
  - Covers status kinds, event triggers, notification policy, and clear/dismiss behavior
  - Start here for `waiting_input` / `waiting_approval` / completion-attention questions
- Contract freeze follow-up:
  - [`../80_decisions/ADR-20260309-sync-v3-contract-freeze.md`](../80_decisions/ADR-20260309-sync-v3-contract-freeze.md)
  - `fixtures/sync-v3/`

## Focused Background

- [20260303-cmux-esc-sequences.md](./20260303-cmux-esc-sequences.md)
  - Narrow `cmux` / Claude hook research
  - Useful background for Claude-specific waiting-state detection
- [claude-jsonl-waiting-states.md](./claude-jsonl-waiting-states.md)
  - Why Claude JSONL cannot provide waiting states

## Historical Design Batch

- [20260302/](./20260302/)
  - Multi-agent Codex JSONL design research that led to the current semantic FSM approach
