# Proposal B: Hybrid ESC + JSONL Architecture for agtmux State Detection

- **Date**: 2026-03-02
- **Status**: Draft Proposal
- **Author**: Research subagent
- **Scope**: Radical redesign of state detection pipeline

---

## 1. cmux Analysis

### 1.1 What cmux Is

cmux is a native macOS terminal application (Swift/AppKit) built on libghostty for GPU-accelerated rendering. It is NOT a tmux wrapper -- it is an alternative terminal emulator with built-in workspace/pane management. This is a fundamental architectural difference from agtmux.

### 1.2 cmux's Detection Mechanisms

cmux uses a **three-layer notification approach**, all of which are fundamentally passive (they rely on the agent or user to emit signals):

| Layer | Mechanism | What It Detects |
|-------|-----------|-----------------|
| **OSC 777** (RXVT protocol) | `\e]777;notify;Title;Body\a` | Simple notification with title+body |
| **OSC 99** (Kitty protocol) | `\e]99;i=ID;e=1;d=0;p=title:Text\e\\` | Rich notification with subtitle, ID, dedup |
| **CLI** (`cmux notify`) | External command: `cmux notify --title "..." --body "..."` | Programmatic notification from hooks/scripts |

### 1.3 What cmux Does NOT Do

Critically, cmux does **not** perform:
- JSONL file parsing or watching
- Process tree inspection
- mtime-based activity heuristics
- State machine inference (Running/Idle/WaitingApproval/WaitingInput)
- Any autonomous agent state detection

cmux's "agent detection" is purely a **notification relay**: when Claude Code's `Stop` or `PostToolUse` hook fires, a user-configured shell script calls `cmux notify`. cmux then shows that notification in its sidebar. This is reactive, not proactive.

### 1.4 cmux vs agtmux: Architectural Implications

| Aspect | cmux | agtmux |
|--------|------|--------|
| **Terminal** | IS the terminal (Ghostty) | Monitors FROM OUTSIDE tmux |
| **Sequence access** | Native -- libghostty callbacks | Must use `pipe-pane` or `capture-pane` |
| **State detection** | None (relies on hooks/notifications) | Active inference from multiple signal sources |
| **Multi-agent** | One workspace = one agent | One tmux server = many agents in many panes |
| **Notification** | OSC 777/99 parsed by terminal engine | Must poll JSONL, hooks, process state |

**Conclusion**: cmux's approach is not directly portable to agtmux. cmux solved a simpler problem (notification relay in a custom terminal) while agtmux must solve a harder one (autonomous state inference from external observation of tmux panes).

---

## 2. ESC Sequence Inventory

### 2.1 Claude Code Emissions (confirmed)

| Sequence | Format | When Emitted | Usefulness for State Detection |
|----------|--------|--------------|-------------------------------|
| **OSC 9;4** (progress) | `\e]9;4;3\a` (indeterminate=running), `\e]9;4;0\a` (done) | During tool execution, marks start/end of work | **HIGH**: state=3 = Running, state=0 = transition to Idle/WaitingInput |
| **OSC 9** (notification) | `\e]9;message text\a` | On task completion, permission requests | **MEDIUM**: indicates completion event, requires tmux passthrough |
| **OSC 2/0** (title) | `\e]2;title\a` | On `/rename` command only | **LOW**: rare, not correlated with activity state |
| **OSC 133** (shell integration) | `\e]133;A\a` through `\e]133;D\a` | **NOT emitted by Claude Code** (shell integration only) | **NONE**: confirmed absent per ADR-20260301 |

### 2.2 Codex CLI Emissions (confirmed)

| Sequence | Format | When Emitted | Usefulness |
|----------|--------|--------------|------------|
| **OSC 9** (notification) | `\e]9;message\a` | Task completion (auto mode), configurable via `tui.notification_method` | **MEDIUM**: completion signal |
| **BEL** (`\x07`) | Bell character | Fallback notification | **LOW**: ambiguous |
| **OSC 8** (hyperlinks) | `\e]8;;URL\a text \e]8;;\a` | File citations in output | **NONE**: informational only |

### 2.3 Key Finding: No WaitingApproval/WaitingInput Sequences

Neither Claude Code nor Codex CLI emit any ESC sequence that signals:
- "I am waiting for user approval" (permission prompt)
- "I am waiting for user input" (idle at prompt)
- "I am in error state"

These states are only detectable via:
- **Claude Code**: `PermissionRequest` hook (WaitingApproval), JSONL event types (WaitingInput implied by `assistant` line + no subsequent `tool_use`)
- **Codex CLI**: JSONL keepalive pattern analysis (unreliable), App Server thread status (`waiting_for_turn` may indicate WaitingInput)

### 2.4 Summary: What ESC Sequences Can Distinguish

```
OSC 9;4 state=3  -->  Running (high confidence)
OSC 9;4 state=0  -->  NOT Running (Idle or WaitingInput -- ambiguous)
OSC 9 text       -->  Completion event (Running -> Idle transition)
[silence]        -->  Ambiguous: could be Idle, WaitingInput, or WaitingApproval
```

ESC sequences alone CANNOT distinguish Idle from WaitingInput from WaitingApproval.

---

## 3. tmux Capture Mechanisms for ESC Sequences

### 3.1 Available Mechanisms

| Mechanism | Command | Raw ESC? | Limitations |
|-----------|---------|----------|-------------|
| **`capture-pane -p`** | `tmux capture-pane -p -t %ID` | **NO** -- strips escape sequences | Only visible text |
| **`capture-pane -p -e`** | `tmux capture-pane -p -e -t %ID` | **Partial** -- preserves ANSI color/SGR | OSC sequences NOT preserved |
| **`pipe-pane`** | `tmux pipe-pane -t %ID 'cat >> /tmp/log'` | **YES** -- raw byte stream | Pre-empts existing pipe-pane; exclusive |
| **`allow-passthrough`** | `set -g allow-passthrough on` | N/A | Forwards sequences to outer terminal, doesn't capture |

### 3.2 pipe-pane: The Only Viable Capture

`pipe-pane` is the only tmux mechanism that preserves OSC sequences. It copies the **raw PTY output** of the pane process to a pipe, including all control sequences before tmux processes them.

**Constraints**:
1. **Exclusive**: Only one `pipe-pane` can be active per pane at a time. Activating a new one cancels the old one.
2. **User conflict**: If the user has their own `pipe-pane` (e.g., for logging), agtmux would break it.
3. **Requires tmux 3.3+** for `allow-passthrough` (but pipe-pane itself works on older tmux).
4. **Data volume**: High-throughput panes (long builds, streaming output) generate substantial pipe data.
5. **Parsing overhead**: Raw terminal output includes all cursor movement, SGR, CSI sequences alongside the OSC sequences of interest.

### 3.3 pipe-pane Architecture

```
[Agent Process]
     |
     v (raw PTY output, includes ESC sequences)
[tmux pane PTY slave]
     |
     +---> [tmux internal rendering] ---> [tmux client terminal]
     |
     +---> [pipe-pane] ---> [agtmux parser process/FIFO]
```

agtmux would need to:
1. Run `tmux pipe-pane -t %ID 'cat >> /tmp/agtmux-osc-%ID.pipe'` for each agent pane
2. Open the FIFO/file and parse the raw byte stream
3. Extract OSC 9;4 sequences while discarding everything else
4. Handle cleanup on pane death/reset

### 3.4 Recommendation

pipe-pane is viable but should remain **Tier 2 (semi-deterministic)** as established in ADR-20260301. The pre-emption risk and user conflict make it unsuitable as the primary detection mechanism. It supplements JSONL and hooks, not replaces them.

---

## 4. WaitingInput Detection: The Critical Analysis

### 4.1 Defining the States

| State | User-facing meaning | From agent's perspective |
|-------|--------------------|-----------------------|
| **Idle** | Session exists but no active task; user hasn't typed anything new | Agent completed response, showing prompt, user hasn't interacted |
| **WaitingInput** | Agent finished its response and is actively showing a prompt | Agent completed response, showing prompt, user hasn't interacted |

**Key insight**: From the agent's perspective, Idle and WaitingInput are **semantically identical**. The agent has finished its work, is showing a prompt, and is waiting. The difference exists only in the UI/UX layer of the monitoring tool.

### 4.2 Detection Approaches

#### Approach A: JSONL Event Analysis

**Claude Code**:
- `type=assistant` line emitted = agent finished response = WaitingInput/Idle
- No subsequent `type=user` line within timeout = Idle
- `type=user` line after `assistant` = was WaitingInput, now transitioning to Running

This gives us a **retroactive** classification: we know WaitingInput existed only after the user types. In real-time, the state after `assistant` is always "possibly WaitingInput".

**Codex CLI**:
- Keepalive writes every ~15s during both task execution AND idle prompt
- JSONL mtime alone cannot distinguish "waiting for user" from "model thinking with keepalive"
- The `session_meta` line indicates session start; subsequent lines indicate activity
- Thread status via App Server: `waiting_for_turn` = WaitingInput (when available)

#### Approach B: Process State Inspection

Check if the agent process is blocked on `read()` (waiting for stdin input):

```bash
# Check stdin file descriptor state
lsof -p <agent_pid> -d 0

# Or check /proc on Linux
cat /proc/<pid>/fdinfo/0   # Check file position
cat /proc/<pid>/wchan       # Should show "poll_schedule_timeout" or "do_select"
```

**macOS limitation**: `/proc` doesn't exist. Must use `lsof` or `dtrace`/`sysctl`.

```bash
# macOS: check what syscall the process is blocked on
sudo dtrace -n 'proc:::signal-send /pid == TARGET/ { trace(arg0); }'
```

**Reliability**: Medium. Agent processes may use async I/O (Node.js event loop), so they're always in `epoll_wait`/`kqueue` regardless of whether they're waiting for user input or doing network I/O. Not useful for Node.js-based agents (Claude Code, Codex).

#### Approach C: OSC 133 (Shell Integration)

If the shell inside the pane emits OSC 133, we could detect:
- `133;A` (prompt start) = shell is showing a prompt
- `133;B` (command start) = user started typing
- `133;C` (command executed) = command is running
- `133;D;exit_code` (command done) = command finished

**Problem**: Claude Code and Codex are TUI applications, not shell commands. OSC 133 comes from the shell integration script (bash/zsh/fish), not from the agent. When an agent is running in a pane, the shell has already exec'd or spawned the agent -- the shell integration is no longer active.

**Exception**: When the agent exits and returns to the shell prompt, OSC 133 would fire again. This detects "agent terminated, back at shell" -- but that's `Terminated`, not `WaitingInput`.

#### Approach D: ESC Sequence Cessation + JSONL Correlation

Combine signals:
1. OSC 9;4 state=3 was seen (Running confirmed)
2. OSC 9;4 state=0 emitted (Running ended)
3. Last JSONL line was `type=assistant` (agent produced response)
4. No new JSONL `type=user` within N seconds

This combination gives the highest confidence for WaitingInput:
- Running confirmed by OSC 9;4
- Completion confirmed by OSC 9;4 state=0
- Response delivered confirmed by `type=assistant` in JSONL
- Waiting confirmed by silence

### 4.3 Recommendation

**WaitingInput is best modeled as a time-decayed state derived from Idle**.

```
Running -> (agent finishes) -> WaitingInput[0s] -> (timeout) -> Idle
                                    ^
                                    |
                     Entry: last event is "completion" + no new user input
                     Exit: user input detected OR session-level timeout
```

**For v1**: Merge WaitingInput and Idle into a single `NotRunning` state with a `last_completed_at` timestamp. Let the UI layer decide how to display it (e.g., "Waiting for input" for the first 5 minutes after completion, "Idle" after that).

**For v2**: Use the hybrid multi-signal approach (Approach D) for higher-confidence WaitingInput detection, primarily valuable when OSC 9;4 pipe-pane is active.

---

## 5. Hybrid Architecture: JSONL + ESC + Process State

### 5.1 Three-Tier Signal Model

```
TIER 1: SEMANTIC (Deterministic)
  |- Claude Hooks (rank 0) -- if configured
  |    PermissionRequest -> WaitingApproval (confidence: 1.0)
  |    SessionStart/End  -> lifecycle transitions
  |    UserPromptSubmit  -> Running entry
  |    PostToolUseFailure -> Error
  |
  |- JSONL Watcher (rank 1) -- always active
  |    type=user          -> activity.user_input  (confidence: 1.0)
  |    type=tool_use      -> activity.running     (confidence: 1.0)
  |    type=tool_result   -> activity.tool_complete (confidence: 1.0)
  |    type=assistant     -> activity.idle        (confidence: 1.0)
  |    type=progress      -> activity.running     (confidence: 1.0)
  |
  |- Codex App Server (rank 1) -- when available
       thread.status      -> Running/Idle/WaitingInput (confidence: 1.0)

TIER 2: OBSERVATIONAL (Semi-deterministic)
  |- OSC 9;4 via pipe-pane (rank 2) -- capability-gated
  |    state=3 (indeterminate) -> Running   (confidence: 0.92)
  |    state=0 (done)          -> NotRunning (confidence: 0.92)
  |    state=1 N%              -> Running N% (confidence: 0.92)
  |
  |- OSC 9 notification         -> completion event (confidence: 0.85)

TIER 3: HEURISTIC (Fallback)
  |- Process inspection (rank 3)
  |    process_hint="claude"/"codex"  -> provider identification
  |    deep tree scan                 -> agent disambiguation
  |
  |- Capture text pattern (rank 3)
       Codex NDJSON in terminal       -> status extraction
       Prompt pattern matching        -> WaitingInput hint
```

### 5.2 Signal Fusion Rules

```rust
fn resolve_activity_state(signals: &[Signal]) -> ActivityState {
    // Priority: highest-tier, highest-confidence signal wins
    // Tie-breaking: most recent signal wins

    // Rule 1: Deterministic signals always override heuristic
    if let Some(det) = signals.iter().filter(|s| s.tier == Deterministic).max_by_key(|s| s.ts) {
        return det.state;
    }

    // Rule 2: OSC 9;4 combined with JSONL context
    if let Some(osc) = signals.iter().find(|s| s.kind == OscProgress) {
        match osc.osc_state {
            3 => return Running,
            0 => {
                // Check JSONL context for Idle vs WaitingInput
                if let Some(jsonl) = last_jsonl_event(signals) {
                    if jsonl.event_type == "activity.idle" && age(jsonl) < WAITING_INPUT_THRESHOLD {
                        return WaitingInput;
                    }
                }
                return Idle;
            }
            _ => {}
        }
    }

    // Rule 3: Heuristic fallback
    heuristic_resolve(signals)
}
```

### 5.3 Data Flow

```
  [tmux pane]
       |
       +-- list-panes ----------> [pane_info] ------+
       |                                             |
       +-- capture-pane --------> [capture_lines] ---+---> [PaneSnapshot]
       |                                             |          |
       +-- pipe-pane (if OSC) --> [osc_parser] ------+          |
       |                                                        v
  [JSONL files] ---> [SessionFileWatcher] ---+           [poll_batch]
       |                                     |                |
  [Claude Hooks] ---> [HookTranslator] ------+---> [Gateway] ---> [DaemonProjection]
       |                                     |
  [Codex AppSrv] --> [AppServerClient] ------+
       |
  [ps -eo ...] ----> [ProcessMap] -------> [deep_inspect]
```

---

## 6. Finite State Machine (FSM) Design

### 6.1 States

```
+------------------+
|   Terminated     |  Pane has no agent process (shell prompt visible)
+------------------+
        |
        v (agent detected via process_hint or JSONL discovery)
+------------------+
|     Idle         |  Agent is present but no active task
|                  |  (> WAITING_TIMEOUT since last completion, or initial state)
+------------------+
        |
        | (JSONL type=user / Hook UserPromptSubmit / AppServer status change)
        v
+------------------+
|    Running       |  Agent is executing a task (tool_use, progress, OSC 9;4 state=3)
+------------------+
        |
        +---------> (Hook PermissionRequest) --> +-------------------+
        |                                         | WaitingApproval  |
        +---------> (JSONL type=assistant /       +-------------------+
        |            OSC 9;4 state=0)                     |
        v                                                 | (Hook PostToolUse / user action)
+------------------+                                      v
| WaitingInput     |  Agent completed response,     (back to Running)
|                  |  awaiting next user message
|                  |  (< WAITING_TIMEOUT)
+------------------+
        |
        | (WAITING_TIMEOUT exceeded, ~5 min)
        v
+------------------+
|     Idle         |
+------------------+

+------------------+
|     Error        |  PostToolUseFailure hook / error JSONL event
+------------------+
        |
        | (next Running event clears error)
        v
     Running
```

### 6.2 Transition Table

| From | To | Trigger Signal | Source Tier | Confidence |
|------|----|----------------|-------------|------------|
| * | **Terminated** | pane_pid disappears from process list | T3 Process | 1.0 |
| Terminated | **Idle** | agent detected (process_hint + JSONL discovery) | T1/T3 | 1.0/0.86 |
| Idle | **Running** | JSONL `type=user` or `type=tool_use` | T1 JSONL | 1.0 |
| Idle | **Running** | Hook `UserPromptSubmit` | T1 Hooks | 1.0 |
| Idle | **Running** | OSC 9;4 state=3 | T2 OSC | 0.92 |
| Idle | **Running** | Codex App Server status=`running` | T1 AppServer | 1.0 |
| Running | **WaitingInput** | JSONL `type=assistant` (last event, no subsequent user) | T1 JSONL | 0.95 |
| Running | **WaitingInput** | OSC 9;4 state=0 + JSONL `type=assistant` | T1+T2 | 0.97 |
| Running | **WaitingInput** | Codex App Server status=`waiting_for_turn` | T1 AppServer | 1.0 |
| Running | **WaitingApproval** | Hook `PermissionRequest` | T1 Hooks | 1.0 |
| Running | **Error** | Hook `PostToolUseFailure` | T1 Hooks | 1.0 |
| Running | **Error** | JSONL error event | T1 JSONL | 1.0 |
| WaitingInput | **Running** | JSONL `type=user` or Hook `UserPromptSubmit` | T1 | 1.0 |
| WaitingInput | **Idle** | WAITING_INPUT_TIMEOUT exceeded (300s) | Timer | 0.80 |
| WaitingApproval | **Running** | Hook `PostToolUse` (approval granted) | T1 Hooks | 1.0 |
| WaitingApproval | **Running** | JSONL `type=tool_use` after approval window | T1 JSONL | 0.95 |
| WaitingApproval | **WaitingInput** | APPROVAL_TIMEOUT exceeded (600s) | Timer | 0.70 |
| Error | **Running** | Any Running trigger | T1/T2 | per signal |
| Error | **WaitingInput** | JSONL `type=assistant` | T1 JSONL | 0.95 |
| * | **Terminated** | Hook `SessionEnd` | T1 Hooks | 1.0 |

### 6.3 Timer-Based Transitions

```
WAITING_INPUT_TIMEOUT  = 300s  (5 min after last assistant message -> Idle)
APPROVAL_TIMEOUT       = 600s  (10 min waiting for approval -> WaitingInput fallback)
ERROR_CLEAR_TIMEOUT    = 120s  (2 min after error with no new events -> Idle)
SESSION_STALE_TIMEOUT  = 3600s (1 hour with no events -> Terminated)
```

---

## 7. Rust Architecture: Struct/Trait Design

### 7.1 New `SourceKind::OscTap`

```rust
// In agtmux-core-v5/src/types.rs
#[non_exhaustive]
pub enum SourceKind {
    CodexAppserver,
    ClaudeHooks,
    ClaudeJsonl,
    OscTap,       // NEW: OSC sequence capture via pipe-pane
    Poller,
}

impl SourceKind {
    pub fn tier(self) -> EvidenceTier {
        match self {
            Self::CodexAppserver | Self::ClaudeHooks | Self::ClaudeJsonl => {
                EvidenceTier::Deterministic
            }
            Self::OscTap => EvidenceTier::SemiDeterministic,  // NEW tier
            Self::Poller => EvidenceTier::Heuristic,
        }
    }
}

// New evidence tier
pub enum EvidenceTier {
    Deterministic,
    SemiDeterministic,  // NEW: higher than heuristic, lower than deterministic
    Heuristic,
}
```

### 7.2 OSC Tap Source Crate

```rust
// crate: agtmux-source-osc-tap

/// Parsed OSC event from pipe-pane stream.
#[derive(Debug, Clone)]
pub enum OscEvent {
    /// OSC 9;4 progress: state 0=done, 1=default, 2=error, 3=indeterminate, 4=warning
    Progress { state: u8, value: Option<u8> },
    /// OSC 9 desktop notification
    Notification { text: String },
    /// OSC 2 window title change
    TitleChange { title: String },
}

/// State for a single pane's OSC tap.
pub struct PaneOscTap {
    pane_id: String,
    /// pipe-pane process handle (None if capability-gated out)
    pipe_process: Option<PipeHandle>,
    /// Ring buffer of recent OSC events
    recent_events: VecDeque<(DateTime<Utc>, OscEvent)>,
    /// Last known progress state
    last_progress_state: Option<u8>,
}

/// OSC byte stream parser.
/// Scans raw terminal output for OSC sequences, discarding everything else.
pub struct OscParser {
    state: ParserState,
    buffer: Vec<u8>,
}

enum ParserState {
    Normal,
    EscSeen,        // ESC received, waiting for ]
    OscBody,        // Inside OSC body, collecting until ST or BEL
}

impl OscParser {
    /// Feed raw bytes from pipe-pane, extract OSC events.
    pub fn feed(&mut self, data: &[u8]) -> Vec<OscEvent> {
        let mut events = Vec::new();
        for &byte in data {
            match self.state {
                ParserState::Normal => {
                    if byte == 0x1B { // ESC
                        self.state = ParserState::EscSeen;
                    }
                }
                ParserState::EscSeen => {
                    if byte == b']' { // ESC ] = OSC start
                        self.state = ParserState::OscBody;
                        self.buffer.clear();
                    } else {
                        self.state = ParserState::Normal;
                    }
                }
                ParserState::OscBody => {
                    if byte == 0x07 || byte == 0x9C { // BEL or ST
                        if let Some(event) = self.parse_osc_body() {
                            events.push(event);
                        }
                        self.buffer.clear();
                        self.state = ParserState::Normal;
                    } else if byte == 0x1B {
                        // Possible ESC \ (ST as two bytes)
                        // Handle in next byte
                        self.state = ParserState::Normal; // simplified
                        if let Some(event) = self.parse_osc_body() {
                            events.push(event);
                        }
                        self.buffer.clear();
                    } else {
                        self.buffer.push(byte);
                    }
                }
            }
        }
        events
    }

    fn parse_osc_body(&self) -> Option<OscEvent> {
        let body = std::str::from_utf8(&self.buffer).ok()?;
        if let Some(rest) = body.strip_prefix("9;4;") {
            // OSC 9;4 progress
            let state: u8 = rest.split(';').next()?.parse().ok()?;
            let value = rest.split(';').nth(1).and_then(|v| v.parse().ok());
            Some(OscEvent::Progress { state, value })
        } else if let Some(text) = body.strip_prefix("9;") {
            // OSC 9 notification
            Some(OscEvent::Notification { text: text.to_string() })
        } else if let Some(title) = body.strip_prefix("2;") {
            // OSC 2 title
            Some(OscEvent::TitleChange { title: title.to_string() })
        } else {
            None // Unrecognized OSC, discard
        }
    }
}

/// Capability check: can this tmux environment support OSC tap?
pub fn check_osc_capability(executor: &impl TmuxCommandRunner) -> OscCapability {
    // Check tmux version >= 3.3
    // Check no existing pipe-pane on target panes
    // Return capability result
    todo!()
}

pub enum OscCapability {
    Available,
    UnavailableTmuxVersion { version: String },
    UnavailablePipePaneConflict { pane_id: String },
}
```

### 7.3 Activity State Resolver

```rust
// In agtmux-daemon-v5 or agtmux-gateway

/// Multi-signal activity state resolver.
/// Replaces the current event_type -> ActivityState mapping with
/// a stateful resolver that considers signal history and timers.
pub struct ActivityStateResolver {
    /// Per-pane state machines
    pane_states: HashMap<String, PaneActivityFsm>,
    /// Configuration
    config: ResolverConfig,
}

pub struct ResolverConfig {
    pub waiting_input_timeout: Duration,   // 300s
    pub approval_timeout: Duration,        // 600s
    pub error_clear_timeout: Duration,     // 120s
    pub session_stale_timeout: Duration,   // 3600s
}

pub struct PaneActivityFsm {
    pub current_state: ActivityState,
    pub entered_at: DateTime<Utc>,
    pub last_signal: Option<Signal>,
    /// History of recent signals (bounded ring buffer)
    pub signal_history: VecDeque<Signal>,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub ts: DateTime<Utc>,
    pub source: SourceKind,
    pub tier: EvidenceTier,
    pub event_type: String,
    pub confidence: f64,
    /// Optional OSC-specific data
    pub osc_progress_state: Option<u8>,
}

impl PaneActivityFsm {
    pub fn apply_signal(&mut self, signal: Signal, config: &ResolverConfig) -> ActivityState {
        let new_state = match (&self.current_state, &signal.event_type) {
            // Deterministic transitions (any current state)
            (_, et) if et == "lifecycle.start" => ActivityState::Idle,
            (_, et) if et == "lifecycle.end" => ActivityState::Idle, // or Terminated

            // Running entries
            (_, et) if et == "activity.user_input" => ActivityState::Running,
            (_, et) if et == "activity.running" => ActivityState::Running,

            // Completion -> WaitingInput
            (ActivityState::Running, et) if et == "activity.idle" => ActivityState::WaitingInput,
            (ActivityState::Running, et) if et == "activity.tool_complete" => {
                // tool_complete is intermediate; stay Running unless followed by idle
                ActivityState::Running
            }

            // WaitingApproval (hooks only)
            (_, et) if et == "activity.waiting_approval" => ActivityState::WaitingApproval,

            // Error
            (_, et) if et == "lifecycle.error" => ActivityState::Error,

            // OSC progress signals
            (_, _) if signal.osc_progress_state == Some(3) => ActivityState::Running,
            (_, _) if signal.osc_progress_state == Some(0) => {
                // Done signal: check JSONL context
                if self.last_jsonl_was_assistant() {
                    ActivityState::WaitingInput
                } else {
                    ActivityState::Idle
                }
            }

            // Default: no change
            _ => self.current_state,
        };

        self.current_state = new_state;
        self.entered_at = signal.ts;
        self.signal_history.push_back(signal.clone());
        self.last_signal = Some(signal);

        // Trim history
        while self.signal_history.len() > 100 {
            self.signal_history.pop_front();
        }

        new_state
    }

    /// Timer-based transitions (called on each poll tick)
    pub fn tick(&mut self, now: DateTime<Utc>, config: &ResolverConfig) -> Option<ActivityState> {
        let age = now - self.entered_at;
        match self.current_state {
            ActivityState::WaitingInput if age > config.waiting_input_timeout => {
                self.current_state = ActivityState::Idle;
                Some(ActivityState::Idle)
            }
            ActivityState::WaitingApproval if age > config.approval_timeout => {
                self.current_state = ActivityState::WaitingInput;
                Some(ActivityState::WaitingInput)
            }
            ActivityState::Error if age > config.error_clear_timeout => {
                self.current_state = ActivityState::Idle;
                Some(ActivityState::Idle)
            }
            _ => None,
        }
    }

    fn last_jsonl_was_assistant(&self) -> bool {
        self.signal_history
            .iter()
            .rev()
            .find(|s| s.source == SourceKind::ClaudeJsonl || s.source == SourceKind::ClaudeHooks)
            .is_some_and(|s| s.event_type == "activity.idle")
    }
}
```

### 7.4 Integration with DaemonState

```rust
// In poll_loop.rs DaemonState
pub struct DaemonState {
    // ... existing fields ...

    /// NEW: OSC tap state per agent pane (Post-MVP)
    pub osc_taps: HashMap<String, PaneOscTap>,

    /// NEW: Activity state resolver (replaces ad-hoc event_type -> state mapping)
    pub activity_resolver: ActivityStateResolver,
}
```

---

## 8. Comparison: JSONL-Only vs Hybrid

### 8.1 What ESC Adds That JSONL Cannot Provide

| Capability | JSONL-Only | With OSC 9;4 | Improvement |
|-----------|------------|--------------|-------------|
| **Running detection latency** | 1-2s (poll interval + JSONL write delay) | ~100ms (pipe-pane stream) | 10-20x faster |
| **Running -> Not Running transition** | JSONL `type=assistant` write (may lag) | OSC 9;4 state=0 (immediate) | Millisecond precision |
| **Codex keepalive disambiguation** | Unreliable (mtime == keepalive or activity) | OSC 9;4 state=3 present = Running, absent = Idle | Eliminates false positives |
| **Progress percentage** | Not available | OSC 9;4 state=1 with progress value | New capability |
| **Provider-agnostic detection** | Requires per-provider JSONL format knowledge | OSC 9;4 is standard across terminals | Simpler for new agents |

### 8.2 What ESC Cannot Replace

| Capability | Available via JSONL/Hooks | Available via ESC | Note |
|-----------|-------------------------|-------------------|------|
| **WaitingApproval** | Hook `PermissionRequest` | Not available | Hooks-only |
| **Session ID** | JSONL `sessionId` field | Not available | JSONL-only |
| **Conversation title** | JSONL `custom-title`/`summary` | Not available | JSONL-only |
| **Transcript path** | Hook `SessionStart` payload | Not available | Hooks-only |
| **Error details** | Hook `PostToolUseFailure` | Not available | Hooks-only |
| **User input content** | JSONL `type=user` message text | Not available | JSONL-only |

### 8.3 Summary

ESC sequences provide **better temporal resolution for Running/NotRunning transitions** but cannot replace JSONL/hooks for **semantic state** (WaitingApproval, session identity, conversation metadata). The optimal architecture uses both:

- **JSONL/Hooks**: ground truth for what the agent is doing and why
- **OSC 9;4**: real-time confirmation/refinement of when state changes occur

---

## 9. Implementation Priority

### Phase 1: JSONL State Machine Enhancement (Immediate, no new infra)

**Goal**: Fix the core problem (WaitingInput/Idle distinction, Codex keepalive) using existing infrastructure.

1. **ActivityStateResolver** with timer-based WaitingInput -> Idle decay (Section 7.3)
   - Replace ad-hoc `event_type` -> `ActivityState` mapping in DaemonProjection
   - Add `WaitingInput` as a real state (currently it exists in types.rs but is never assigned from JSONL)
   - `Running -> WaitingInput` on `type=assistant` JSONL line
   - `WaitingInput -> Idle` after 300s timeout

2. **Codex keepalive disambiguation** via JSONL content analysis
   - Parse the actual JSONL line content (not just mtime) to distinguish keepalive from real activity
   - Codex keepalive lines have predictable structure; real activity lines have unique content

3. **WaitingApproval** state from hooks
   - Already mapped in `translate.rs` (`PermissionRequest` -> `activity.waiting_approval`)
   - Ensure DaemonProjection propagates this to the API response

**Effort**: ~2-3 days. No new crates. No pipe-pane dependency.

### Phase 2: OSC 9;4 Tap (Post-MVP, capability-gated)

**Goal**: Add real-time Running detection for environments that support it.

1. **`agtmux-source-osc-tap` crate** (Section 7.2)
   - OscParser for raw byte stream
   - PaneOscTap lifecycle management
   - Capability detection (tmux version, pipe-pane availability)

2. **pipe-pane orchestration**
   - Start/stop pipe-pane per agent pane
   - Handle cleanup on pane death
   - Respect `--no-osc-tap` flag for users who have their own pipe-pane

3. **Integration with ActivityStateResolver**
   - OSC signals feed into the same resolver as JSONL/hooks
   - OSC 9;4 state=3 -> Running (confidence 0.92)
   - OSC 9;4 state=0 -> confirmation of JSONL idle transition

**Effort**: ~5-7 days. New crate. Requires integration testing with real tmux.

### Phase 3: Cross-Signal Fusion (Future)

**Goal**: Leverage multiple simultaneous signals for highest-confidence detection.

1. **Confidence scoring** when multiple signals disagree
2. **Anomaly detection** (JSONL says idle but OSC says running -> investigate)
3. **Provider-agnostic OSC detection** for new agents (Gemini CLI, Aider, etc.)

**Effort**: ~3-5 days. Depends on Phase 2.

### Dependency Graph

```
Phase 1: JSONL State Machine Enhancement
    |
    v (Phase 1 complete)
Phase 2: OSC 9;4 Tap
    |
    v (Phase 2 complete)
Phase 3: Cross-Signal Fusion
```

Phase 1 is **independent and immediately valuable**. Phase 2 adds incremental improvement. Phase 3 is the long-term vision.

---

## Appendix A: OSC 9;4 Format Reference

```
ESC ] 9 ; 4 ; <state> ; <progress> BEL

States:
  0 = Hidden (done)           -> progress bar hidden, task complete
  1 = Default                 -> show progress at <progress>% (0-100)
  2 = Error                   -> show progress bar in error state
  3 = Indeterminate           -> show spinning/pulsing indicator (no percentage)
  4 = Warning                 -> show progress bar in warning state

Examples:
  \e]9;4;3\a          -> Task running (indeterminate)
  \e]9;4;1;50\a       -> 50% progress
  \e]9;4;0\a          -> Task done
  \e]9;4;2;75\a       -> 75% progress, error state
```

## Appendix B: OSC 133 (Shell Integration) Reference

```
ESC ] 133 ; A ST    -> Prompt start (FTCS_PROMPT)
ESC ] 133 ; B ST    -> Command start / prompt end (FTCS_COMMAND_START)
ESC ] 133 ; C ST    -> Command output start (FTCS_COMMAND_EXECUTED)
ESC ] 133 ; D ST    -> Command done (FTCS_COMMAND_FINISHED)
ESC ] 133 ; D ; N   -> Command done with exit code N

NOT emitted by Claude Code or Codex CLI.
Only emitted by shell integration scripts (bash/zsh/fish).
GitHub issue #26235 is an open feature request for Claude Code to emit these.
```

## Appendix C: cmux Notification OSC Reference

```
OSC 777 (RXVT):
  \e]777;notify;Title;Body\a

OSC 99 (Kitty):
  \e]99;i=<id>;e=1;d=0;p=title:<text>\e\\

OSC 9 (simple notification):
  \e]9;<text>\a

cmux CLI:
  cmux notify --title "..." --body "..." [--subtitle "..."]
```

## Appendix D: Sources

- [cmux GitHub repository](https://github.com/manaflow-ai/cmux)
- [cmux notification documentation](https://www.cmux.dev/docs/notifications)
- [OSC 9;4 progress sequences (Microsoft Terminal docs)](https://github.com/MicrosoftDocs/terminal/blob/main/TerminalDocs/tutorials/progress-bar-sequences.md)
- [Codex CLI notification configuration](https://developers.openai.com/codex/config-advanced/)
- [OSC 133 shell integration (Contour)](https://contour-terminal.org/vt-extensions/osc-133-shell-integration/)
- [tmux pipe-pane documentation](https://github.com/tmux/tmux/wiki/Advanced-Use)
- [tmux allow-passthrough configuration](https://tmuxai.dev/tmux-allow-passthrough/)
- [iTerm2 OSC escape codes](https://iterm2.com/documentation-escape-codes.html)
- [Kitty OSC 9;4 support issue](https://github.com/kovidgoyal/kitty/issues/3679)
- [Claude Code status line documentation](https://code.claude.com/docs/en/statusline)
- [Codex CLI macOS notification setup](https://samwize.com/2026/02/05/setup-codex-cli-notifications-on-macos-iterm2-terminal-notifier/)
