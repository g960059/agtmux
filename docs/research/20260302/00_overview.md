# agtmux Codex Detection — Research & Redesign
Date: 2026-03-02

## Project Context

agtmux is a Rust daemon monitoring tmux panes running AI agents (Claude Code, Codex CLI).
It classifies pane states and displays them in the status bar.

Current version: **v0.1.12** (released 2026-03-02)

## Problem Statement

The Codex state detection is fundamentally broken due to mtime ambiguity:

| Write type          | mtime behavior | Actual state     |
|---------------------|----------------|------------------|
| task execution burst | recent mtime  | Running          |
| keepalive heartbeat  | recent mtime  | Idle/WaitingInput|

mtime alone cannot distinguish these. v0.1.12 is a partial fix — it prevents false-positive
"Running" but cannot detect Running at all for sessions in old date directories.

## Goal

Radical redesign (no backward compat) that correctly classifies:
- **Running**: actively executing a task
- **Idle**: session completed, no active task, no prompt shown
- **WaitingApproval**: paused waiting for tool permission grant
- **WaitingInput**: printed response, showing input prompt, awaiting user message
- **Error**: exception / unexpected state

## Research Team

| Agent | Focus | Status |
|-------|-------|--------|
| Agent A (Claude Opus) | JSONL formats + Proposal A (JSONL-first) | Running in background |
| Agent B (Claude Opus) | ESC sequences + Proposal B (Hybrid) | Running in background |
| Codex C | JSONL schema + FSM design | Running in background |
| Codex D | ESC sequences + terminal integration | Running in background |

## Key Finding: cmux Approach

cmux (https://github.com/manaflow-ai/cmux) uses **OSC 9/99/777 sequences** for state notification.
- Agents emit these sequences OR call `cmux notify` via hooks
- States: Waiting-for-input (blue ring), Active/Running, Idle
- NOT relying on JSONL parsing — sequence-based push model

This suggests a hybrid approach: hooks/sequences for Claude Code, JSONL for Codex.

## Files in This Directory

| File | Description |
|------|-------------|
| 00_overview.md | This file |
| 01_bug_investigation.md | v0.1.9 → v0.1.12 bug timeline |
| 02_proposals_comparison.md | 3-agent initial proposals (pre-team) |
| 03_unified_design_v0.md | Consensus design v0 |
| 04_research_questions.md | Open questions for research team |
| agent-A-proposal.md | Claude Opus A output (TBD) |
| agent-B-proposal.md | Claude Opus B output (TBD) |
| codex-proposal-C.md | Codex C output (TBD) |
| codex-proposal-D.md | Codex D output (TBD) |
| 05_synthesis.md | Final synthesis after team results (TBD) |
