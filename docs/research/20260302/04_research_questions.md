# Research Questions for Team

## P1 (Critical): Codex JSONL Schema

### 1a. Complete Event Type List
What are ALL event types in Codex JSONL?
- Known: `session_meta`, `event_msg` with subtypes `turn/started`, `turn/completed`, `waitingOnApproval`
- Unknown: Are there `turn/delta`, `response_item`, `turn/paused`, or similar?
- What do keepalive/heartbeat lines look like structurally?
  - Is keepalive a `session_meta` repeat? A distinct type? Empty object?
  - Frequency: exactly 15s? Variable?

### 1b. WaitingInput Signal
**This is the most important open question.**
- After `turn/completed`, Codex shows its response and presents ">" input prompt
- At this point: is there a JSONL event? Or does the JSONL simply stop?
- Is WaitingInput = Idle from JSONL perspective? (just no new events)
- Or is there a specific `waitingOnInput` event?

### 1c. WaitingApproval Sequence
- Full JSONL sequence: `turn/started` → ... → `waitingOnApproval` → (user grants) → ?
- After user grants approval: what event? `turn/started` again? `approval_granted`?
- Is approval request visible in JSONL before the `waitingOnApproval` event?

### 1d. Error/Abort Events
- If user presses Ctrl+C: what JSONL event?
- If Codex crashes: what happens to the JSONL?
- `turn/aborted` or similar?

## P2 (Important): Claude Code JSONL Schema

### 2a. Full Event Type Inventory
- What events exist beyond the basics in `crates/agtmux-source-claude-jsonl/`?
- Is there a `waiting_approval` event in Claude JSONL?
- Is there a `waiting_input` event?

### 2b. Current Coverage
- Does the existing `agtmux-source-claude-jsonl` handle WaitingApproval?
- Are there states that the current Claude source is missing?
- What gaps exist in Claude vs Codex source symmetry?

## P3 (Important): ESC Sequences

### 3a. Codex ESC Sequences
- What ESC/OSC sequences does Codex CLI emit to the terminal?
- Any progress indicators? Status sequences?
- Does Codex emit OSC 9;4 (progress) like Claude Code does?
- Does Codex emit anything when showing the ">" input prompt?

### 3b. Claude Code ESC Sequences (Known)
- OSC 9;4 → `Ps=4;0` (done), `Ps=4;1` (set), `Ps=4;2` (error), `Ps=4;3` (indeterminate)
- OSC 9 → bell notification
- OSC 2/0 → window title (on `/rename` command)
- Does OSC 9;4;1 (set = working) correlate with Running state?
- Does OSC 9;4;0 (done) correlate with turn/completed → Idle?

### 3c. OSC 133 (Shell Integration)
- OSC 133;A = prompt start (shell waiting for input)
- OSC 133;B = command start
- OSC 133;C = command end
- OSC 133;D = command complete
- Are these emitted by the SHELL running Claude/Codex, or by the agents themselves?
- Can OSC 133;A (prompt start) indicate WaitingInput for the AI agent?

### 3d. tmux Capture Mechanisms
Which tmux mechanism gives access to ESC sequences from pane output?
- `tmux pipe-pane`: raw output including ESC sequences
- `tmux capture-pane -e`: attributes but not raw ESC sequences
- `tmux set-option allow-passthrough on`: passes sequences through to outer terminal
- For reading sequences programmatically: which is correct?

## P4 (Important): cmux Analysis

### 4a. cmux Technical Details
From today's research: cmux uses OSC 9/99/777 and `cmux notify` CLI.
- What exactly is OSC 99? (OSC 9 is bell, OSC 777 is often used for Urxvt notifications)
- Is OSC 777 a custom/convention sequence, or standardized?
- Does cmux require agents to call `cmux notify` explicitly?

### 4b. Claude Code cmux Compatibility
- Claude Code hooks (Stop, PostToolUse, etc.) can call `cmux notify`
- This is a push-notification model: agent calls cmux when state changes
- Does this require modifying the Claude Code hook script?
- How does cmux distinguish WaitingInput from Idle via OSC sequences?

### 4c. Codex cmux Compatibility
- Does Codex CLI have a hook system like Claude Code?
- If not: can Codex be configured to call `cmux notify` on state changes?
- Or must JSONL parsing provide the state for Codex?

## Design Questions

### D1. WaitingInput vs Idle
Should WaitingInput be a separate state from Idle?
- **Option A**: They're the same state (both mean "no active task")
  - Simpler, no ambiguity about when WaitingInput ends
  - cmux only shows "waiting for input" as the attention state
- **Option B**: Separate states with distinct UI treatment
  - WaitingInput: agent just finished, prompt visible, user should respond
  - Idle: session dormant, maybe no active context
  - How to transition: WaitingInput → Idle after timeout? After user types?

**Hypothesis**: Idle = no session or session dormant; WaitingInput = session active, post-turn/completed

### D2. Discovery Frequency
- Run `lsof -p <pid> -d cwd` every tick (1s)?
- Cache CWD for N ticks and only re-discover if watcher loses file?
- Performance: lsof for N panes × M ticks/s = CPU cost

### D3. Multiple Sessions Per Pane
- One pane might restart Codex multiple times, creating multiple JSONL files
- How to handle: track only most recent? Track all?
- What if two sessions are "active" simultaneously (rare but possible)?

### D4. Claude Code Source Enhancements
- Should `agtmux-source-claude-jsonl` gain the same FSM (WaitingApproval, WaitingInput)?
- Currently Claude Code uses hooks which already provide good state signal
- Should both sources eventually converge to the same FSM?

## Research Team Assignments

| Team Member | Primary | Secondary |
|-------------|---------|-----------|
| Agent A (Claude Opus) | P1 (Codex JSONL schema) + P2 | D1, D2 |
| Agent B (Claude Opus) | P3 (ESC sequences) + P4 | D3, D4 |
| Codex C | P1 + D1 (JSONL → FSM design) | D2 |
| Codex D | P3 + P4 (ESC + cmux) | D1 (ESC-based detection) |

## Success Criteria

The research team should produce answers to:
1. Is `turn/completed` the only transition to WaitingInput/Idle? Or is there a distinct signal?
2. Can ESC sequences supplement JSONL for real-time detection in Codex?
3. What is the minimum viable FSM for correct Codex state classification?
4. Is there any Codex signal that distinguishes WaitingInput from Idle?
