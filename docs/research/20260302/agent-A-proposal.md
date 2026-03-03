# Proposal A: `agtmux-source-codex-jsonl` — JSONL-First Architecture
*Agent A (Claude Opus) — 2026-03-02*

## 1. Actual Codex JSONL Event Schema (from reading real files)

### File Location
```
~/.codex/sessions/YYYY/MM/DD/rollout-<ISO-ts>-<UUIDv7>.jsonl
```

### Complete Event Type Inventory

| Top-level `type` | Nested `payload.type` | Description | FSM-relevant? |
|---|---|---|---|
| `session_meta` | -- | Line 1 always. Contains `payload.cwd`, `payload.id`, `payload.cli_version`, `payload.source` ("exec"), `payload.model_provider` | Yes (init) |
| `response_item` | `message` (role=developer) | Sandbox permissions, system instructions | No (metadata) |
| `response_item` | `message` (role=user) | User task text, AGENTS.md injections | No (metadata) |
| `response_item` | `message` (role=assistant) | Agent's text response | Yes (reasoning/output) |
| `response_item` | `reasoning` | Agent's internal reasoning | No (metadata) |
| `response_item` | `function_call` | Tool invocation (`name=exec_command`, etc.) | **Yes (ToolExecuting)** |
| `response_item` | `function_call_output` | Tool execution result | **Yes (back to Running)** |
| `event_msg` | `task_started` | Turn begins. Contains `turn_id`, `model_context_window`, `collaboration_mode_kind` | **Yes (→ Running)** |
| `event_msg` | `task_complete` | Turn ends. Contains `turn_id`, `last_agent_message` | **Yes (→ WaitingInput)** |
| `event_msg` | `user_message` | User's prompt text | Yes (input event) |
| `event_msg` | `agent_reasoning` | Agent's intermediate reasoning text | No (granular) |
| `event_msg` | `agent_message` | Agent's commentary or final message | No (granular) |
| `event_msg` | `token_count` | Token usage update | No (metadata) |
| `turn_context` | -- | Turn metadata: `turn_id`, `cwd`, `approval_policy` ("never" or "on-request"), `sandbox_policy`, `model`, `effort` | **Yes (approval policy)** |

### KEY DISCOVERIES (Correcting Previous Assumptions)

> **⚠️ CRITICAL: No keepalive/heartbeat lines exist in Codex JSONL.**
>
> The "keepalive write" behavior previously observed was actually `agent_reasoning` and `token_count`
> events written during long-running inference, plus OS-level metadata updates. When Codex is truly
> waiting for user input, **the file simply stops growing**. This completely changes the v0.1.11/v0.1.12
> analysis — Pass 3 was fighting a ghost.

> **⚠️ No explicit `waitingOnApproval` event exists in JSONL.**
>
> The approval policy is declared in `turn_context.payload.approval_policy` ("on-request" or "never"),
> but there is no discrete "waiting for user approval" event. Codex waits for terminal input interactively
> — the JSONL file simply stops receiving writes.

### Inter-turn Timeline (from real data)
```
task_complete  [08:17:20.364Z]   <-- Turn 1 ends
                                   ~5h40m gap (user away)
task_started   [13:57:24.627Z]   <-- Turn 2 starts
```

## 2. Full FSM Transition Table

### States

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSessionState {
    Init,            // Session JSONL discovered but no events parsed yet
    Running,         // Agent actively processing (between task_started and task_complete)
    WaitingInput,    // Agent completed turn, waiting for user to type next prompt
    ToolExecuting,   // Agent executing a tool (function_call seen, no function_call_output yet)
    Ended,           // Session ended (process exited or staleness timeout)
}
```

### Transition Table

| Current State | Event | New State | Notes |
|---|---|---|---|
| `Init` | `session_meta` | `Init` | Record CWD, session_id |
| `Init` | `event_msg:task_started` | `Running` | First turn begins |
| `Running` | `response_item:function_call` | `ToolExecuting` | Agent invoked a tool |
| `Running` | `event_msg:agent_reasoning` | `Running` | No state change |
| `Running` | `event_msg:agent_message` | `Running` | No state change |
| `Running` | `event_msg:token_count` | `Running` | No state change |
| `Running` | `event_msg:task_complete` | `WaitingInput` | Turn done, waiting for user |
| `ToolExecuting` | `response_item:function_call_output` | `Running` | Tool finished |
| `ToolExecuting` | `response_item:function_call` | `ToolExecuting` | Parallel tool call |
| `ToolExecuting` | `event_msg:task_complete` | `WaitingInput` | Edge case |
| `WaitingInput` | `event_msg:task_started` | `Running` | User submitted new prompt |
| `WaitingInput` | (staleness > threshold OR process exit) | `Ended` | Session dormant |
| `*` | `turn_context` | (no change) | Record approval_policy |
| `*` | `event_msg:user_message` | (no change) | Metadata only |

### WaitingApproval Detection (Heuristic)

No JSONL event for this. Proposed approach:
- If `approval_policy == "on-request"` AND FSM is `ToolExecuting` AND no `function_call_output`
  arrives for >5 seconds → emit `WaitingApproval`
- For `--full-auto` sessions (approval_policy="never"), WaitingApproval is impossible

## 3. Rust Type Definitions

### discovery.rs

```rust
pub struct CodexPaneHint {
    pub pane_id: String,
    pub pane_pid: u32,
    pub cwd: String,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct CodexSessionDiscovery {
    pub pane_id: String,
    pub session_id: String,
    pub jsonl_path: PathBuf,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
}

/// Algorithm:
/// 1. lsof -p <pane_pid> -d cwd -Fn → get CWD (always-open fd, timing-independent)
/// 2. Walk ~/.codex/sessions/ recursively across ALL date directories (no filter)
/// 3. Read line 1 of each .jsonl → parse session_meta → extract payload.cwd
/// 4. Match by CWD (canonicalized)
/// 5. Return most-recently-modified match
pub fn discover_codex_session(hint: &CodexPaneHint) -> Option<CodexSessionDiscovery>;
```

### watcher.rs

```rust
pub struct CodexFileWatcher {
    path: PathBuf,
    byte_offset: u64,
    inode: u64,
    partial_line: String,
    bootstrapped: bool,
    task_text: Option<String>,
    approval_policy: Option<String>,
}

impl CodexFileWatcher {
    pub fn new(path: PathBuf) -> Self;
    pub fn new_from_start(path: PathBuf) -> Self;
    pub fn poll_new_lines(&mut self) -> Vec<String>;
}
```

### fsm.rs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSessionState { Init, Running, WaitingInput, ToolExecuting, Ended }

pub struct CodexJsonlEvent {
    pub timestamp: Option<DateTime<Utc>>,
    pub top_type: String,
    pub inner_type: Option<String>,
    pub role: Option<String>,
    pub payload: serde_json::Value,
}

pub struct CodexSessionFsm {
    state: CodexSessionState,
    last_transition_at: Option<DateTime<Utc>>,
    cwd: Option<String>,
    approval_policy: String,
    pending_tool_calls: u32,
}

impl CodexSessionFsm {
    pub fn transition(&mut self, event: &CodexJsonlEvent) -> Option<CodexSessionState>;
    pub fn state(&self) -> CodexSessionState;
    pub fn approval_policy(&self) -> &str;
}
```

### translate.rs

```rust
fn state_to_event_type(state: CodexSessionState) -> &'static str {
    match state {
        CodexSessionState::Init         => "activity.idle",
        CodexSessionState::Running      => "activity.running",
        CodexSessionState::WaitingInput => "activity.waiting_input",
        CodexSessionState::ToolExecuting => "activity.running",   // same as Running in UI
        CodexSessionState::Ended        => "activity.idle",
    }
}
```

## 4. WaitingInput State Detection — DEFINITIVE

**Yes, WaitingInput IS detectable from JSONL alone:**
- Signal: `event_msg:task_complete` → FSM enters WaitingInput
- End: next `event_msg:task_started` → FSM enters Running
- **No keepalive ambiguity** — the file simply stops growing when waiting

| Property | WaitingInput | Ended |
|---|---|---|
| Last JSONL event | `task_complete` | `task_complete` (same!) |
| Codex process alive | Yes | No |
| Detection | FSM state after task_complete | Process exit OR staleness timeout |

**Key differentiator**: Check process liveness via process_map. If codex process exits after task_complete → transition to Ended.

## 5. Trade-offs vs Current Approach

| Aspect | Current | Proposed |
|---|---|---|
| States | 2 (active/idle) | 5 (Running, WaitingInput, ToolExecuting, Ended, Init) |
| Timing accuracy | 25s mtime windows | Sub-second event-driven |
| Date dirs | today+yesterday only | All directories |
| App Server | JSON-RPC client, 4s timeout | Eliminated entirely |
| WaitingInput | Cannot detect | Yes, via task_complete |
| WaitingApproval | Cannot detect | Heuristic (approval_policy + timeout) |
| Complexity | ~700 LOC codex_poller.rs | ~400 LOC, 5 focused modules |

## 6. Migration Plan

### DELETE from `codex_poller.rs`
- `CodexAppServerClient` + all methods
- `classify_notloaded_status()`
- `scan_jsonl_sessions()` + Pass 1/2/3
- `find_open_jsonl_for_pid()`, `find_codex_child_jsonl()`
- `CodexCaptureTracker`, `parse_codex_capture_events()`
- Constants: `HEARTBEAT_INTERVAL_SECS`, `NOTLOADED_*_THRESHOLD_SECS`, `JSONL_*_THRESHOLD_SECS`, `MAX_CWD_QUERIES_PER_TICK`

### DELETE from `poll_loop.rs`
- `DaemonState.codex_appserver_client`
- `DaemonState.codex_supervisor`
- Step 6a App Server poll block (~120 lines)
- Step 6a-bis `scan_jsonl_sessions` call

### ADD
```
crates/agtmux-source-codex-jsonl/
  Cargo.toml
  src/
    lib.rs
    discovery.rs   — CWD via lsof -d cwd → session walk all date dirs
    watcher.rs     — byte-offset tracking (same as claude-jsonl watcher)
    fsm.rs         — Init/Running/WaitingInput/ToolExecuting/Ended
    translate.rs   — FSM state → SourceEventV2
    source.rs      — Source trait impl, poll loop bridge
```

## 7. Open Questions

1. **Full dir walk frequency**: Cache with 30s TTL, or inotify/kqueue on sessions dir?
2. **Multiple JSONL per pane**: Use most-recently-modified match (already proposed)
3. **WaitingApproval heuristic**: 5s timeout during ToolExecuting — is this reliable enough?
4. **Staleness threshold for Ended**: 600s? Configurable?
5. **SourceKind migration**: Add `CodexJsonl` alongside `CodexAppserver` or clean break?
