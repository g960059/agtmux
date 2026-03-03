# Unified Design v0: agtmux-source-codex-jsonl

> Status: DRAFT — based on 3-agent consensus, pre-research-team
> Will be superseded by v1 after research team completes

## Overview

Replace the entire current Codex detection stack with a new source crate that
does semantic JSONL parsing, analogous to `agtmux-source-claude-jsonl`.

## Module Breakdown

### discovery.rs

```rust
/// Find the JSONL file(s) for a given tmux pane.
pub fn discover_jsonl_for_pane(pane_pid: u32) -> Vec<PathBuf> {
    // Step 1: Get pane CWD via lsof -p <pid> -d cwd -Fn
    // The CWD fd (fd=cwd) is ALWAYS open — timing-independent
    let cwd = get_cwd_via_lsof(pane_pid)?;

    // Step 2: Walk ~/.codex/sessions/**/*.jsonl (ALL date dirs, no filter)
    let sessions_dir = home_dir().join(".codex/sessions");
    let jsonl_files = glob::glob(&format!("{}/**/*.jsonl", sessions_dir))?;

    // Step 3: Match by CWD in session_meta (line 1)
    jsonl_files
        .filter(|path| read_session_meta_cwd(path) == Some(&cwd))
        .collect()
}
```

**Key**: `lsof -p <pid> -d cwd -Fn` output format:
```
p1234
fcwd
n/path/to/cwd
```
Parse `n` lines to get CWD path.

### watcher.rs

```rust
pub struct JsonlWatcher {
    path: PathBuf,
    inode: u64,
    byte_offset: u64,
    partial_line: String,
}

impl JsonlWatcher {
    pub fn poll(&mut self) -> Vec<CodexJsonlLine> {
        // 1. Check inode (detect file rotation/recreation)
        let current_inode = stat(&self.path).inode();
        if current_inode != self.inode {
            self.inode = current_inode;
            self.byte_offset = 0;
            self.partial_line.clear();
        }

        // 2. Open file, seek to byte_offset
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.byte_offset))?;

        // 3. Read new bytes
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        self.byte_offset += buf.len() as u64;

        // 4. Split on '\n', handle partial last line
        let text = String::from_utf8_lossy(&buf);
        let mut lines = Vec::new();
        for part in text.split('\n') {
            if part.is_empty() { continue; }
            let full = format!("{}{}", self.partial_line, part);
            self.partial_line.clear();
            if text.ends_with('\n') || part != text.split('\n').last().unwrap() {
                if let Ok(line) = serde_json::from_str(&full) {
                    lines.push(line);
                }
            } else {
                self.partial_line = full; // incomplete line
            }
        }
        lines
    }
}
```

### fsm.rs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexState {
    #[default]
    Idle,
    Running,
    WaitingApproval,
    WaitingInput,  // TBD: detection mechanism unclear
    Error,
}

pub fn transition(state: CodexState, line: &CodexJsonlLine) -> CodexState {
    match line {
        CodexJsonlLine::EventMsg { event_type, .. } => match event_type.as_str() {
            "turn/started"        => CodexState::Running,
            "turn/completed"      => CodexState::Idle,
            "waitingOnApproval"   => CodexState::WaitingApproval,
            // waitingOnInput? TBD — needs research
            _ => state, // unknown events preserve state
        },
        CodexJsonlLine::SessionMeta { .. } => state, // keepalive-safe: no transition
        CodexJsonlLine::Keepalive { .. }   => state, // preserve state
    }
}
```

**FSM Transition Table** (v0, incomplete):

| From \ Event         | turn/started | turn/completed | waitingOnApproval | session_meta | unknown |
|----------------------|--------------|----------------|-------------------|--------------|---------|
| Idle                 | Running      | Idle           | WaitingApproval   | Idle         | Idle    |
| Running              | Running      | Idle           | WaitingApproval   | Running      | Running |
| WaitingApproval      | Running      | Idle           | WaitingApproval   | WApproval    | WApproval |
| WaitingInput         | Running      | Idle           | WaitingApproval   | WInput       | WInput  |
| Error                | Running      | Idle           | WaitingApproval   | Error        | Error   |

> Note: WaitingInput transition OUT requires research (cmux uses OSC 9/99/777?)

### translate.rs

```rust
pub fn to_source_event(state: CodexState, pane_id: &str, session_id: &str) -> SourceEventV2 {
    let event_type = match state {
        CodexState::Running         => "thread.active",
        CodexState::Idle            => "thread.idle",
        CodexState::WaitingApproval => "thread.waiting_approval",
        CodexState::WaitingInput    => "thread.waiting_input",
        CodexState::Error           => "thread.error",
    };
    SourceEventV2 {
        event_type: event_type.to_string(),
        pane_id: Some(pane_id.to_string()),
        session_id: session_id.to_string(),
        is_heartbeat: false,
        ..Default::default()
    }
}
```

### source.rs

```rust
pub struct CodexJsonlSource {
    watchers: HashMap<PaneId, Vec<JsonlWatcher>>,
}

impl Source for CodexJsonlSource {
    fn poll(&mut self, pane_infos: &[PaneCwdInfo]) -> Vec<SourceEventV2> {
        let mut events = Vec::new();

        for pane in pane_infos {
            let pane_pid = pane.pane_pid?;

            // Discovery: find JSONL files for this pane
            let jsonl_paths = discover_jsonl_for_pane(pane_pid);

            // Update watchers
            let entry = self.watchers.entry(pane.pane_id.clone()).or_default();
            // ... sync watchers with jsonl_paths ...

            // Poll watchers, run FSM
            for watcher in entry.iter_mut() {
                for line in watcher.poll() {
                    let new_state = transition(watcher.current_state, &line);
                    if new_state != watcher.current_state {
                        watcher.current_state = new_state;
                        events.push(to_source_event(new_state, &pane.pane_id, &watcher.session_id));
                    }
                }
            }
        }

        events
    }
}
```

## What to Delete

From `crates/agtmux-runtime/src/codex_poller.rs`:
- `CodexAppServerClient` struct + all methods
- `classify_notloaded_status()` function
- `scan_jsonl_sessions()` + Pass 1/2/3 logic
- `MAX_CWD_QUERIES_PER_TICK` constant
- `JSONL_IDLE_THRESHOLD_SECS` constant
- `poll_app_server()` function
- `is_file_write_open()` (already removed in v0.1.10)

From `crates/agtmux-runtime/src/poll_loop.rs`:
- Step 6a-bis (scan_jsonl_sessions call)
- Any App Server integration steps

## Design Questions Still Open

1. **WaitingInput**: What JSONL event / OSC sequence indicates this state?
   - cmux uses OSC 9/99/777 but these need to be emitted BY the agent
   - Does Codex emit any such sequence when showing its prompt?
   - Can `lsof -p <pid> -d 0` (stdin) reliably detect this?

2. **Multiple JSONL files per pane**: Is it possible to have 2+ active sessions?
   - If yes: which one takes priority?
   - Use most-recently-modified? Or last-with-turn/started?

3. **Discovery frequency**: Run lsof every tick (1s)? Cache CWD for N ticks?

4. **Fail-closed**: If lsof fails, show Unknown state or last-known state?

## Version History

- v0 (2026-03-02): Initial consensus from 3-agent round
- v1 (TBD): After research team completes (Agent A/B, Codex C/D)
