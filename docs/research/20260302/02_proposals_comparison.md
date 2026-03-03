# Three-Agent Proposal Comparison (Pre-Research-Team Round)

## Context

Three independent agents produced proposals for radical Codex detection redesign
(before the current research team was assembled):
- Orchestrator (Claude Sonnet): synthesis proposal
- Claude Subagent: independent proposal
- Codex Agent: independent proposal

## Points of Full Consensus

All three agents agreed on these core design decisions:

| Decision | Rationale |
|----------|-----------|
| New crate: `agtmux-source-codex-jsonl` | Analogous to `agtmux-source-claude-jsonl` |
| Semantic JSONL parsing (not mtime) | Only way to distinguish execution vs keepalive |
| Byte-offset tracking | Incremental reads, no duplicate processing |
| PID-based discovery via `lsof -d cwd` | CWD fd always open, timing-independent |
| No date directory filtering | Cover historical sessions |
| turn/started → Running | Explicit semantic event |
| turn/completed → Idle | Explicit semantic event |
| waitingOnApproval → WaitingApproval | Explicit semantic event |
| keepalive lines → no-op | Preserve current state |
| Delete App Server thread/list polling | Returns notLoaded, causes keepalive oscillation |
| Delete scan_jsonl_sessions() Pass 1/2/3 | Replace entirely |

## Unique Contributions Per Agent

### Orchestrator (Sonnet) Proposal
- 5-file crate structure mirroring claude-jsonl: discovery/watcher/fsm/translate/source
- Emphasis on `lsof -d cwd` as timing-independent (vs `lsof` on open files)
- Clean architecture matching existing source pattern

### Claude Subagent Proposal
- Explicit note: CWD fd from `lsof -p <pid> -d cwd` is **always open** (unlike regular file fds)
- **App Server廃止 completely** (Approach D) — vs others who wanted to keep notifications
- `classify_line()` state machine with detailed pseudocode
- `fail-closed binding`: if pane can't be determined → don't assign JSONL to any pane

### Codex Agent Proposal
- **WaitingApproval as explicit 3rd state** (others noted it but Codex was most explicit)
- **Fail-closed binding**: Codex stressed this most strongly
- **App Server notifications only**: keep for push-notification events, discard thread/list
- **JSONL replay for consistency**: re-read from beginning on session reconnect
- Explicit note: `lsof -d 0` (stdin) could detect WaitingInput (process blocked on read())

## Divergences

### On App Server
- Orchestrator/Sonnet: Remove entirely
- Codex Agent: Keep for push notifications (turn/started/completed), remove thread/list
- **Resolution**: Remove entirely (v0.1.12 shows App Server is unreliable anyway)

### On WaitingInput
- All agreed: WaitingInput is distinct from Idle conceptually
- None had a definitive JSONL signal for it
- Codex Agent suggested: `lsof -p <pid> -d 0` — if stdin open and process blocked
- **Open question**: requires cmux/ESC sequence research to resolve

### On Discovery Fallback
- Orchestrator: Primary is lsof -d cwd, fallback is CWD-based walk
- Claude Subagent: Only lsof-d-cwd, no fallback (fail-closed)
- Codex Agent: lsof -d cwd primary, accept that some sessions may not be bindable

## Proposed Crate Structure (v0 consensus)

```
agtmux-source-codex-jsonl/
  Cargo.toml
  src/
    lib.rs
    discovery.rs   — pane_pid → lsof -d cwd → JSONL walk (all date dirs)
    watcher.rs     — inode + byte_offset + partial_line_buf
    fsm.rs         — Idle/Running/WaitingApproval/WaitingInput/Error FSM
    translate.rs   — FSM state → SourceEventV2
    source.rs      — poll_loop bridge
```

## Open Questions After Round 1

1. **WaitingInput detection**: What signal? JSONL event? ESC sequence? Process state?
2. **cmux approach**: OSC 9/99/777 — do Claude Code / Codex emit these?
3. **Exact JSONL schema**: What do keepalive lines look like? All event types?
4. **waitingOnApproval sequence**: JSONL sequence before/after approval?
5. **ESC supplement**: Can OSC sequences (especially OSC 9;4 from Claude Code) supplement JSONL?

## cmux Finding (from today's research)

cmux (macOS terminal app) uses:
- OSC 9/99/777 sequences for state notification
- Agents emit these OR call `cmux notify` via hooks
- States: Waiting-for-input (blue ring), Active/Running, Idle

This suggests for Claude Code (which already has hook support):
- PostToolUse/PreToolUse hooks can push state via OSC or similar
- Claude Code already emits OSC 9;4 (progress) — correlates with Running

For Codex: JSONL semantic parsing remains the best approach.
