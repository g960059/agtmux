# Synthesis: 4-Agent Research Team Results
*Compiled 2026-03-02 — Claude Sonnet (Orchestrator)*

## Status: All 4 agents complete

| Agent | Status | Key Contribution |
|-------|--------|-----------------|
| Agent A (Claude Opus) | ✅ | Full JSONL schema, WaitingInput=task_complete, no keepalive myth |
| Agent B (Claude Opus) | ✅ | cmux = libghostty (not portable), pipe-pane for ESC, timer-based WaitingInput |
| Codex C (gpt-5.3) | ✅ | **1130 files analyzed** — complete event taxonomy, JSON key is `.payload.type` |
| Codex D (gpt-5.3) | ✅ | agtmux-tmux-v5 only uses capture-pane, pipe-pane is Phase 2 |

---

## Critical Corrections to Prior Assumptions

### 1. JSON key is `.payload.type`, NOT `.data.type`
The original bug report assumed `{"type":"event_msg","data":{"type":"turn/started"}}`.
**Reality (from Codex C, 1130 files)**: `{"type":"event_msg","payload":{"type":"task_started"}}`.

The event names also differ:
| Old assumption | Actual event |
|---|---|
| `turn/started` | `task_started` |
| `turn/completed` | `task_complete` |
| `waitingOnApproval` | `entered_review_mode` |

### 2. `entered_review_mode` / `exited_review_mode` ARE the WaitingApproval signals!
These events appear 46 times each across 1130 session files.
- `entered_review_mode` → WaitingApproval state
- `exited_review_mode` → back to Running (or Idle if approved/rejected)
**No heuristic needed** — explicit JSONL events exist.

### 3. No keepalive/heartbeat lines exist
The "keepalive write every ~15s" hypothesis was WRONG.
- When Codex is waiting for user input, the JSONL file simply stops growing
- The "keepalive writes" we observed were `token_count` and `agent_reasoning` events during long-running inference
- **This means v0.1.11/v0.1.12 were fighting a ghost** — Pass 3 oscillation was from something else

### 4. Additional event types discovered
```
turn_aborted (100 occurrences)        → interruption (Ctrl+C)
context_compacted (151 occurrences)   → context window compaction
entered_review_mode (46 occurrences)  → WaitingApproval ←←← KEY
exited_review_mode (46 occurrences)   → exit WaitingApproval
item_completed (8 occurrences)        → unknown
thread_rolled_back (1 occurrence)     → unknown
```

---

## Complete Event Schema (Verified from 1130 real JSONL files)

### Top-level event types (frequency)
```
162210  event_msg       → agent events
147529  response_item   → LLM output items
 37286  turn_context    → turn metadata
  1130  session_meta    → session metadata (line 1)
   151  compacted       → context compaction marker
```

### event_msg.payload.type (frequency)
```
108370  token_count           → token usage stats (no FSM transition)
 37443  agent_reasoning       → chain-of-thought (no FSM transition)
 10441  agent_message         → agent output text (no FSM transition)
  2502  user_message          → user input received (→ Running if from WaitingInput)
  1785  task_started          → *** Running entry signal ***
  1561  task_complete         → *** WaitingInput entry signal ***
   151  context_compacted     → (no FSM transition)
   100  turn_aborted          → *** interruption signal ***
    46  exited_review_mode    → *** WaitingApproval exit signal ***
    46  entered_review_mode   → *** WaitingApproval entry signal ***
     8  item_completed        → (TBD)
     1  thread_rolled_back    → (TBD)
```

### response_item.payload.type (frequency)
```
 42968  function_call          → *** ToolExecuting entry signal ***
 42941  function_call_output   → *** ToolExecuting exit signal (back to Running) ***
 36737  reasoning              → chain-of-thought (no FSM transition)
 16699  message                → LLM message (role: user/assistant/developer)
  3729  custom_tool_call_output → tool result (MCP/custom tools)
  3729  custom_tool_call        → tool invocation (MCP/custom tools)
   910  web_search_call         → web search
    42  ghost_snapshot          → git snapshot (no FSM transition)
```

---

## Definitive FSM Design

### States

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexSessionState {
    #[default]
    Init,             // Session discovered, no events yet
    Running,          // task_started received
    ToolExecuting,    // function_call received, awaiting function_call_output
    WaitingApproval,  // entered_review_mode received
    WaitingInput,     // task_complete received, process alive
    Ended,            // Process exited OR staleness timeout
}
```

### FSM Transition Table (VERIFIED)

| Current State | Event | New State |
|---|---|---|
| Init | `session_meta` | Init (record CWD) |
| Init | `task_started` | Running |
| Running | `function_call` | ToolExecuting |
| Running | `custom_tool_call` | ToolExecuting |
| Running | `entered_review_mode` | WaitingApproval |
| Running | `task_complete` | WaitingInput |
| Running | `turn_aborted` | WaitingInput (interrupted) |
| Running | `token_count`/`agent_reasoning`/`agent_message` | Running (no change) |
| ToolExecuting | `function_call_output` | Running |
| ToolExecuting | `custom_tool_call_output` | Running |
| ToolExecuting | `entered_review_mode` | WaitingApproval |
| ToolExecuting | `task_complete` | WaitingInput |
| WaitingApproval | `exited_review_mode` | Running |
| WaitingApproval | `task_complete` | WaitingInput |
| WaitingInput | `task_started` | Running (new turn) |
| WaitingInput | `user_message` | Running (user submitted) |
| WaitingInput | (process exit) | Ended |
| WaitingInput | (staleness > 600s) | Ended |
| Any | `task_started` | Running |
| Any | (pane exits) | Ended |

### WaitingInput vs Idle vs Ended

| State | Codex process | Last JSONL | File growing |
|---|---|---|---|
| WaitingInput | Alive | `task_complete` | No |
| Idle (= Ended) | Dead | `task_complete` | No |
| Running | Alive | `task_started` | Yes |

**Key differentiator**: WaitingInput = FSM state after `task_complete` while process is alive.
Transition to Ended when process exits OR after 600s staleness timeout.

---

## Final Architecture: `agtmux-source-codex-jsonl`

### Module Structure

```
crates/agtmux-source-codex-jsonl/
  Cargo.toml
  src/
    lib.rs
    discovery.rs   — pane_pid → lsof -d cwd → JSONL walk (all date dirs)
    watcher.rs     — inode + byte_offset + partial_line_buf
    fsm.rs         — Init/Running/ToolExecuting/WaitingApproval/WaitingInput/Ended
    translate.rs   — FSM state → SourceEventV2
    source.rs      — Source trait impl, poll loop bridge
```

### discovery.rs Algorithm

```
1. lsof -p <pane_pid> -d cwd -Fn → get CWD (ALWAYS open fd, timing-independent)
2. Walk ~/.codex/sessions/**/*.jsonl (ALL date dirs, no date filter)
3. Read line 1 of each .jsonl → parse session_meta → extract payload.cwd
4. Match by CWD (canonicalized with /tmp→/private/tmp)
5. Return most-recently-modified match
```

**Important**: JSON key is `.payload.cwd` not `.data.cwd`.

### watcher.rs Algorithm

```
1. Open file, seek to byte_offset
2. Read new bytes
3. Split on '\n', buffer partial last line
4. Return complete lines
5. Detect inode change → reset to byte 0
```

No keepalive lines to worry about. File stops growing = WaitingInput.

### fsm.rs Transition Function

```rust
pub fn transition(state: CodexSessionState, event: &CodexJsonlEvent) -> CodexSessionState {
    let inner = event.inner_type.as_deref().unwrap_or("");
    match (state, event.top_type.as_str(), inner) {
        // task lifecycle
        (_, "event_msg", "task_started")       => Running,
        (Running | ToolExecuting, "event_msg", "task_complete")  => WaitingInput,
        (Running | ToolExecuting, "event_msg", "turn_aborted")   => WaitingInput,
        (WaitingInput, "event_msg", "task_started") => Running,
        (WaitingInput, "event_msg", "user_message") => Running,

        // tool execution
        (Running, "response_item", "function_call")         => ToolExecuting,
        (Running, "response_item", "custom_tool_call")      => ToolExecuting,
        (ToolExecuting, "response_item", "function_call_output")         => Running,
        (ToolExecuting, "response_item", "custom_tool_call_output")      => Running,

        // approval
        (_, "event_msg", "entered_review_mode") => WaitingApproval,
        (WaitingApproval, "event_msg", "exited_review_mode") => Running,

        // no state change
        _ => state,
    }
}
```

### What to DELETE

From `crates/agtmux-runtime/src/codex_poller.rs`:
- `CodexAppServerClient` + all JSON-RPC code
- `classify_notloaded_status()`
- `scan_jsonl_sessions()` + Pass 1/2/3
- `find_open_jsonl_for_pid()`, `find_codex_child_jsonl()`
- `CodexCaptureTracker`, `parse_codex_capture_events()`
- All constants: `MAX_CWD_QUERIES_PER_TICK`, `JSONL_IDLE_THRESHOLD_SECS`, `HISTORICAL_ENRICHMENT_SECS`, etc.

From `crates/agtmux-runtime/src/poll_loop.rs`:
- DaemonState fields: `codex_appserver_client`, `codex_supervisor`, `codex_capture_tracker`
- Step 6a App Server poll block
- Step 6a-bis scan_jsonl_sessions call

---

## cmux Analysis Summary

**cmux is a native macOS terminal (libghostty) — not a tmux wrapper.**
It does NOT perform autonomous state detection. Its "agent detection" is:
- User configures hooks (Stop, PostToolUse) to call `cmux notify` CLI
- cmux shows visual notification (blue ring) in its sidebar
- Sequences: OSC 777 (RXVT), OSC 99 (Kitty), OSC 9 (simple)

**Why cmux's approach is not directly portable**:
- cmux IS the terminal → direct sequence access
- agtmux MONITORS tmux from outside → must use pipe-pane
- cmux relies on agents emitting sequences or calling its CLI
- agtmux uses autonomous detection, no agent changes required

**What agtmux can learn from cmux**:
- Hook integration (`cmux notify` = our PostToolUse/Stop hooks) is already done
- WaitingInput in cmux = "needs attention" (simplification of our state model)
- Timer-based decay: cmux shows notification until user interacts

---

## ESC Sequence Findings Summary

| Sequence | Emitter | When | State Correlation | tmux Capture |
|---|---|---|---|---|
| OSC 9;4;3 | Claude Code | Tool executing | Running (0.92 conf) | pipe-pane only |
| OSC 9;4;0 | Claude Code | Tool done | Running→NotRunning | pipe-pane only |
| OSC 9 | Claude Code, Codex | Task complete | Weak idle signal | pipe-pane only |
| BEL | Codex | Notifications | Completion hint | pipe-pane only |
| OSC 133 | Shell only | Prompt | Shell prompt | NOT from agents |

**Current tmux-v5 implementation**: only `capture-pane` (strips ESC sequences).
**To capture ESC**: must add `pipe-pane` support to agtmux-tmux-v5.

---

## Implementation Plan

### Phase 1: `agtmux-source-codex-jsonl` (Immediate)
**Goal**: Fix Codex detection completely for all current bugs.
- New crate with discovery/watcher/fsm/translate/source
- Correct JSON key: `.payload.type` not `.data.type`
- Correct events: `task_started`, `task_complete`, `entered_review_mode`, `exited_review_mode`
- Delete: App Server, mtime-based code, Pass 1/2/3
- **WaitingApproval**: deterministic via `entered_review_mode` (no heuristic needed!)
- **WaitingInput**: deterministic via `task_complete` + process-alive check
- Effort: ~3-5 days

### Phase 2: OSC 9;4 Tap via pipe-pane (Post-MVP)
**Goal**: Faster Running detection, especially for Claude Code.
- Add `pipe_pane_start/stop` to agtmux-tmux-v5
- New `agtmux-source-osc-tap` crate
- Capability-gated (tmux 3.3+, no pipe conflict)
- Supplements JSONL, does not replace
- Effort: ~5-7 days

### Phase 3: Enhanced Claude Code State (Future)
**Goal**: Symmetric state model for Claude Code and Codex.
- Add WaitingApproval detection to Claude JSONL source (hook-based)
- Add WaitingInput state to Claude source
- Unify FSM across providers
- Effort: ~2-3 days

---

## Immediate Next Steps

1. **Create `crates/agtmux-source-codex-jsonl/`** with 5 modules
2. **Fix JSON key**: everywhere `data.type` → `payload.type` in new code
3. **Update event names**: task_started, task_complete, entered_review_mode, exited_review_mode
4. **Wire into poll_loop.rs**: replace Step 6a/6a-bis with new source poll
5. **Delete**: CodexAppServerClient + scan_jsonl_sessions + mtime code
6. **Test**: write e2e tests for all 5 states

---

## Files Reference

| File | Purpose |
|------|---------|
| `00_overview.md` | Project overview |
| `01_bug_investigation.md` | v0.1.9→v0.1.12 bug timeline |
| `02_proposals_comparison.md` | Initial 3-agent comparison |
| `03_unified_design_v0.md` | Design v0 (now superseded by this) |
| `04_research_questions.md` | Questions sent to research team |
| `agent-A-proposal.md` | Claude Opus A full proposal |
| `agent-B-proposal.md` | Claude Opus B full proposal (ESC focus) |
| `codex-proposal-C.md` | Codex C (JSONL schema from real data) |
| `codex-proposal-D.md` | Codex D (ESC/terminal integration) |
| `05_synthesis.md` | This file — final synthesis |
