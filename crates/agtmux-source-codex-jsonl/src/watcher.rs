//! File watcher for Codex JSONL session files.
//!
//! Tracks seek position per session file, handles partial lines,
//! and detects file rotation via inode changes.

use chrono::{DateTime, Utc};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::fsm::{CodexJsonlEvent, CodexSessionState, transition};

/// Watcher for a single Codex JSONL session file.
#[derive(Debug)]
pub struct CodexSessionFileWatcher {
    path: PathBuf,
    /// Current byte offset into the file.
    seek_pos: u64,
    /// Inode number (for rotation detection).
    inode: u64,
    /// Incomplete line buffer from previous read (partial line at EOF).
    incomplete_buffer: String,
    /// True after the first poll.
    bootstrapped: bool,
    /// FSM state reconstructed from the historical scan performed at watcher creation.
    historical_state: CodexSessionState,
    /// Timestamp of the last state-changing historical event, if present.
    last_event_ts: Option<DateTime<Utc>>,
    /// First non-injected user prompt seen in this session, used as a title fallback.
    last_first_prompt: Option<String>,
}

impl CodexSessionFileWatcher {
    /// Create a new watcher, seeking to EOF so that future lines are tracked.
    pub fn new(path: PathBuf) -> Self {
        let (seek_pos, inode) = file_metadata(&path).unwrap_or((0, 0));
        let (historical_state, last_event_ts, last_first_prompt) = if seek_pos > 0 {
            scan_historical(&path)
        } else {
            (CodexSessionState::Init, None, None)
        };
        Self {
            path,
            seek_pos,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
            historical_state,
            last_event_ts,
            last_first_prompt,
        }
    }

    /// Create a watcher starting from byte 0 (for testing).
    #[cfg(test)]
    pub fn new_from_start(path: PathBuf) -> Self {
        let inode = file_metadata(&path).map(|(_, ino)| ino).unwrap_or(0);
        Self {
            path,
            seek_pos: 0,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
            historical_state: CodexSessionState::Init,
            last_event_ts: None,
            last_first_prompt: None,
        }
    }

    pub fn is_bootstrapped(&self) -> bool {
        self.bootstrapped
    }

    pub fn mark_bootstrapped(&mut self) {
        self.bootstrapped = true;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn historical_state(&self) -> CodexSessionState {
        self.historical_state
    }

    pub fn last_event_ts(&self) -> Option<DateTime<Utc>> {
        self.last_event_ts
    }

    pub fn last_first_prompt(&self) -> Option<&str> {
        self.last_first_prompt.as_deref()
    }

    pub fn observe_prompt_line(&mut self, line: &str) {
        if self.last_first_prompt.is_none() {
            self.last_first_prompt = extract_first_prompt(line);
        }
    }

    /// Poll for new complete lines since last read.
    pub fn poll_new_lines(&mut self) -> Vec<String> {
        // Check for inode change (file rotation)
        if let Some((_, new_inode)) = file_metadata(&self.path) {
            if self.inode != 0 && new_inode != self.inode {
                self.seek_pos = 0;
                self.inode = new_inode;
                self.incomplete_buffer.clear();
            } else {
                self.inode = new_inode;
            }
        }

        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) => {
                warn!(path = %self.path.display(), error = %e, "failed to open Codex JSONL file");
                return Vec::new();
            }
        };

        let mut reader = BufReader::new(file);
        if let Err(e) = reader.seek(SeekFrom::Start(self.seek_pos)) {
            warn!(
                path = %self.path.display(),
                offset = self.seek_pos,
                error = %e,
                "failed to seek in Codex JSONL file"
            );
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut buf = String::new();

        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if buf.ends_with('\n') {
                        let mut line = std::mem::take(&mut self.incomplete_buffer);
                        line.push_str(buf.trim_end_matches('\n'));
                        if !line.is_empty() {
                            lines.push(line);
                        }
                    } else {
                        self.incomplete_buffer.push_str(&buf);
                    }
                }
                Err(e) => {
                    warn!(path = %self.path.display(), error = %e, "error reading Codex JSONL file");
                    break;
                }
            }
        }

        if let Ok(pos) = reader.stream_position() {
            self.seek_pos = pos;
        }

        lines
    }
}

fn file_metadata(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).ok().map(|m| (m.len(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).ok().map(|m| (m.len(), 0))
    }
}

fn scan_historical(path: &Path) -> (CodexSessionState, Option<DateTime<Utc>>, Option<String>) {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return (CodexSessionState::Init, None, None),
    };

    let mut state = CodexSessionState::Init;
    let mut last_event_ts = None;
    let mut first_prompt = None;
    let reader = BufReader::new(file);

    for line in reader.lines().map_while(Result::ok) {
        if first_prompt.is_none() {
            first_prompt = extract_first_prompt(&line);
        }
        let Ok(event) = serde_json::from_str::<CodexJsonlEvent>(&line) else {
            continue;
        };
        let new_state = transition(state, &event);
        if new_state != state {
            state = new_state;
            if let Some(ts) = parse_timestamp(&line) {
                last_event_ts = Some(ts);
            }
        }
    }

    (state, last_event_ts, first_prompt)
}

fn parse_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let timestamp = value["timestamp"].as_str()?;
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|ts| ts.with_timezone(&Utc))
}

fn extract_first_prompt(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;

    let candidate = match value["type"].as_str() {
        Some("response_item")
            if value["payload"]["type"].as_str() == Some("message")
                && value["payload"]["role"].as_str() == Some("user") =>
        {
            value["payload"]["content"]
                .as_array()
                .and_then(|items| {
                    items.iter().find_map(|item| match item["type"].as_str() {
                        Some("input_text") | Some("text") => item["text"].as_str(),
                        _ => None,
                    })
                })
                .map(str::to_owned)
        }
        Some("event_msg") if value["payload"]["type"].as_str() == Some("user_message") => {
            value["payload"]["message"].as_str().map(str::to_owned)
        }
        _ => None,
    }?;

    let trimmed = candidate.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('<') {
        return None;
    }

    Some(truncate_prompt(trimmed, 120))
}

fn truncate_prompt(prompt: &str, max_chars: usize) -> String {
    let mut chars = prompt.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_jsonl(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("agtmux-test-codex-watcher");
        fs::create_dir_all(&dir).expect("test");
        dir.join(name)
    }

    #[test]
    fn watcher_reads_new_lines() {
        let path = temp_jsonl("codex-test-read-lines.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = CodexSessionFileWatcher::new_from_start(path.clone());

        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .expect("test");

        let lines = watcher.poll_new_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("task_started"));
        assert!(lines[1].contains("task_complete"));

        // Second poll — no new lines
        let lines2 = watcher.poll_new_lines();
        assert!(lines2.is_empty());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_skips_partial_line() {
        let path = temp_jsonl("codex-test-partial.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = CodexSessionFileWatcher::new_from_start(path.clone());
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        write!(f, r#"{{"type":"event_msg","pay"#).expect("test");

        let lines = watcher.poll_new_lines();
        assert!(lines.is_empty(), "partial line should not be returned");

        writeln!(f, r#"load":{{"type":"task_started"}}}}"#).expect("test");
        let lines2 = watcher.poll_new_lines();
        assert_eq!(lines2.len(), 1);
        assert!(lines2[0].contains("task_started"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_seeks_to_eof_on_new() {
        let path = temp_jsonl("codex-test-eof-seek.jsonl");
        fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"type":"session_meta","cwd":"/tmp/test"}}
{"type":"event_msg","payload":{"type":"task_started"}}
"#,
        )
        .expect("test");

        let mut watcher = CodexSessionFileWatcher::new(path.clone());
        let lines = watcher.poll_new_lines();
        assert!(
            lines.is_empty(),
            "new() should seek to EOF — no historical lines"
        );
        assert_eq!(watcher.historical_state(), CodexSessionState::Running);

        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .expect("test");

        let lines2 = watcher.poll_new_lines();
        assert_eq!(lines2.len(), 1);
        assert!(lines2[0].contains("task_complete"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_tracks_historical_waiting_input_state_and_timestamp() {
        let path = temp_jsonl("codex-test-historical-state.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-03-06T04:48:31.769Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-03-06T04:48:49.314Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
        )
        .expect("test");

        let watcher = CodexSessionFileWatcher::new(path.clone());
        assert_eq!(watcher.historical_state(), CodexSessionState::WaitingInput);
        assert_eq!(
            watcher.last_event_ts(),
            Some(
                DateTime::parse_from_rfc3339("2026-03-06T04:48:49.314Z")
                    .expect("valid timestamp")
                    .with_timezone(&Utc)
            )
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_extracts_first_prompt_from_history_and_skips_injected_lines() {
        let path = temp_jsonl("codex-test-first-prompt.jsonl");
        fs::write(
            &path,
            concat!(
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /tmp/work"}]}}"##,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"count lines in /etc/hosts and write the count to result.txt"}}"#
            ),
        )
        .expect("test");

        let watcher = CodexSessionFileWatcher::new(path.clone());
        assert_eq!(
            watcher.last_first_prompt(),
            Some("count lines in /etc/hosts and write the count to result.txt")
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_observe_prompt_line_captures_first_real_prompt_only() {
        let path = temp_jsonl("codex-test-observe-prompt.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = CodexSessionFileWatcher::new_from_start(path.clone());
        watcher.observe_prompt_line(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>"}]}}"#,
        );
        watcher.observe_prompt_line(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"reply with probe-ok"}]}}"#,
        );
        watcher.observe_prompt_line(
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"should not overwrite first prompt"}}"#,
        );

        assert_eq!(watcher.last_first_prompt(), Some("reply with probe-ok"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_handles_rotation() {
        let path = temp_jsonl("codex-test-rotation.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = CodexSessionFileWatcher::new_from_start(path.clone());
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_started"}}}}"#
        )
        .expect("test");
        drop(f);
        let lines = watcher.poll_new_lines();
        assert_eq!(lines.len(), 1);

        // Simulate rotation via rename (guarantees different inode)
        let new_path = path.with_extension("new");
        let mut f2 = fs::File::create(&new_path).expect("test");
        writeln!(
            f2,
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .expect("test");
        drop(f2);
        fs::rename(&new_path, &path).expect("rename");

        let lines2 = watcher.poll_new_lines();
        assert!(
            !lines2.is_empty(),
            "should read from new file after rotation"
        );
        assert!(lines2[0].contains("task_complete"));

        let _ = fs::remove_file(&path);
    }
}
