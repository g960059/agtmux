# Bug Investigation: v0.1.9 → v0.1.12

## Timeline

### v0.1.9 Bugs Identified

**Bug 1 — False positive (vm agtmux panes × 3)**:
- Multiple panes shared the same CWD
- All three showed "Running" (only one had an active Codex session)
- Root cause: CWD-based matching cannot distinguish which pane owns the JSONL

**Bug 2 — False negative (test-session)**:
- test-session Codex was Running but showed "Idle"
- JSONL was in `/2026/02/23/` — older than today/yesterday scan range
- Root cause: date directory restriction cut off historical sessions

### v0.1.10 Fix (Process-first detection)

**Change**: Added Pass 1 using `lsof -p <codex_child_pid> -Fan` to find open JSONL files
- pane_pid → process_map → find Codex child → lsof → JSONL path
- CWD-based scan demoted to fallback (Pass 2)
- `is_file_write_open()` removed

**Remaining issue**:
- Codex burst-writes: `open → write → close` cycle is very short
- During the closed period, lsof returns nothing → Pass 1 empty
- Pass 2 (today/yesterday) still misses old date dirs

### v0.1.11 Fixes (2026-03-02)

Three independent changes:

1. **MAX_CWD_QUERIES_PER_TICK**: 1 → 3
   - With multiple CWDs, cap at 1 caused starvation
   - vm-agtmux CWD always got queried, test-session CWD never did

2. **poll_threads outer timeout**: 8s → 16s
   - Accommodate 3 CWD queries within budget

3. **Pass 3 mtime-based active detection** (CAUSED REGRESSION):
   - Removed `age_secs <= JSONL_IDLE_THRESHOLD_SECS` skip
   - Added active/idle branching: `age <= 25s → thread.active`
   - Reasoning: "fresh mtime in old date dir = Running"

### v0.1.11 Regression: updatedAt Oscillation

**Symptom**: test-session updatedAt oscillated 9–16s (increased then immediately decreased)

**Sequence of events**:
```
T=0s:   Codex keepalive write → mtime fresh
T=1s:   Pass 3 sees age<25s → emits thread.active → updatedAt = "just now"
T=15s:  Codex keepalive write again → mtime fresh
T=16s:  Pass 3 sees age<25s → emits thread.active → updatedAt = "just now" again
... cycles every 9-16s
```

**Root cause**: v0.1.11 removed the skip that was preventing this exact problem.
Codex writes keepalive every ~15s while waiting for user input.
mtime cannot distinguish this from "task is executing".

**User report**: "test sessionのcodexのupdatedAtが9~16sの間で増えては減ってという不思議な挙動"

### v0.1.12 Revert (2026-03-02)

**Changes**:
- Restored `age_secs <= JSONL_IDLE_THRESHOLD_SECS` skip in Pass 3
- Pass 3 always emits `thread.idle` (reverted active/idle branching)
- Added detailed comment explaining keepalive-write limitation
- Kept MAX_CWD_QUERIES_PER_TICK = 3 (starvation fix retained)
- Kept outer timeout = 16s (retained)

**Final state (v0.1.12)**:
```
Pass 1: lsof -p <codex_pid> → JSONL (timing-dependent, often empty)
Pass 2: today/yesterday mtime scan → Running/Idle by recency
Pass 3: historical 7-day, skip fresh files → always thread.idle
App Server: notLoaded for all, mtime re-classifies
```

**Remaining issue**: test-session in `/2026/02/23/` still shows Idle when Running.
Cannot be fixed within the mtime paradigm.

## Root Cause Summary

### The Fundamental Mtime Problem

```
Codex write pattern during task execution:
  JSONL: ████░░████░░████░░████  (burst-write with closed gaps)
  mtime: recent throughout

Codex write pattern while waiting for user input:
  JSONL: ░░░░░░█░░░░░░░░░░█░░░░  (sparse keepalive, ~15s interval)
  mtime: recent during keepalive
```

**mtime is indistinguishable between these two cases.**

The only reliable signal is the **JSONL content** itself:
- `turn/started` event → task execution started
- `turn/completed` event → task execution done
- `waitingOnApproval` event → paused for permission

### What v0.1.12 Does NOT Solve

1. Sessions in old date directories (before yesterday) remain stuck as Idle
2. Sessions during burst-write closed gaps are invisible to Pass 1
3. WaitingApproval state is not tracked at all
4. WaitingInput state is not tracked at all

## Required: Semantic JSONL Parsing

The only correct fix is to read and parse Codex JSONL content — not just mtime.
This requires a new `agtmux-source-codex-jsonl` crate (see unified_design_v0.md).
