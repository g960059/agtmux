//! File watcher for Claude JSONL transcript files.
//!
//! Tracks seek position per session file, handles partial lines,
//! and detects file rotation via inode changes.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tracing::warn;

/// Maximum byte length of a conversation title extracted from JSONL content.
const MAX_TITLE_LEN: usize = 200;

/// Watcher for a single JSONL session file.
#[derive(Debug)]
pub struct SessionFileWatcher {
    path: PathBuf,
    /// Current byte offset into the file.
    seek_pos: u64,
    /// Inode number (for rotation detection).
    inode: u64,
    /// Incomplete line buffer from previous read (partial line at EOF).
    incomplete_buffer: String,
    /// True after the first poll — guards the one-shot bootstrap event emission.
    bootstrapped: bool,
    /// Latest custom-title seen in this JSONL file (T-135b).
    last_title: Option<String>,
    /// Latest AI-generated summary seen in this JSONL file (T-135c).
    last_summary: Option<String>,
    /// First user prompt text extracted from this JSONL file (lowest-priority title fallback).
    last_first_prompt: Option<String>,
}

impl SessionFileWatcher {
    /// Create a new watcher for the given JSONL file path.
    ///
    /// Seeks to EOF so that future activity events are tracked from now on.
    /// Performs a one-time historical scan of existing content to backfill
    /// `last_title`, `last_summary`, and `last_first_prompt` — ensuring that
    /// sessions already running at daemon start still get a conversation title.
    pub fn new(path: PathBuf) -> Self {
        let (seek_pos, inode) = match file_metadata(&path) {
            Some((size, ino)) => (size, ino),
            None => (0, 0),
        };

        let mut last_title = None;
        let mut last_summary = None;
        let mut last_first_prompt = None;

        if seek_pos > 0 {
            scan_historical(
                &path,
                seek_pos,
                &mut last_title,
                &mut last_summary,
                &mut last_first_prompt,
            );
        }

        Self {
            path,
            seek_pos,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
            last_title,
            last_summary,
            last_first_prompt,
        }
    }

    /// Whether the first-poll bootstrap event has already been emitted.
    pub fn is_bootstrapped(&self) -> bool {
        self.bootstrapped
    }

    /// Mark the watcher as having completed its bootstrap.
    pub fn mark_bootstrapped(&mut self) {
        self.bootstrapped = true;
    }

    /// Return the latest custom-title seen in this JSONL file (T-135b).
    pub fn last_title(&self) -> Option<&str> {
        self.last_title.as_deref()
    }

    /// Update the latest custom-title for this session (T-135b).
    pub fn set_title(&mut self, title: String) {
        self.last_title = Some(title);
    }

    /// Return the latest AI-generated session summary (T-135c).
    pub fn last_summary(&self) -> Option<&str> {
        self.last_summary.as_deref()
    }

    /// Update the latest session summary (T-135c).
    pub fn set_summary(&mut self, summary: String) {
        self.last_summary = Some(summary);
    }

    /// Return the first user prompt text seen in this JSONL file (lowest-priority fallback).
    pub fn last_first_prompt(&self) -> Option<&str> {
        self.last_first_prompt.as_deref()
    }

    /// Set the first user prompt (only stored if not already set — first message wins).
    pub fn set_first_prompt(&mut self, prompt: String) {
        if self.last_first_prompt.is_none() {
            self.last_first_prompt = Some(prompt);
        }
    }

    /// Create a watcher starting from position 0 (for testing).
    #[cfg(test)]
    pub fn new_from_start(path: PathBuf) -> Self {
        let inode = file_metadata(&path).map(|(_, ino)| ino).unwrap_or(0);
        Self {
            path,
            seek_pos: 0,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
            last_title: None,
            last_summary: None,
            last_first_prompt: None,
        }
    }

    /// Poll for new complete lines since last read.
    ///
    /// Returns a list of complete lines (without trailing newline).
    /// Partial lines at EOF are buffered until the next poll.
    pub fn poll_new_lines(&mut self) -> Vec<String> {
        // Check for file rotation (inode change)
        if let Some((_, new_inode)) = file_metadata(&self.path) {
            if self.inode != 0 && new_inode != self.inode {
                // File was rotated — reset to beginning
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
                warn!(path = %self.path.display(), error = %e, "failed to open JSONL file");
                return Vec::new();
            }
        };

        let mut reader = BufReader::new(file);
        if let Err(e) = reader.seek(SeekFrom::Start(self.seek_pos)) {
            warn!(
                path = %self.path.display(),
                offset = self.seek_pos,
                error = %e,
                "failed to seek in JSONL file"
            );
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut buf = String::new();

        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if buf.ends_with('\n') {
                        // Complete line
                        let mut line = std::mem::take(&mut self.incomplete_buffer);
                        line.push_str(buf.trim_end_matches('\n'));
                        if !line.is_empty() {
                            lines.push(line);
                        }
                    } else {
                        // Partial line at EOF — buffer for next poll
                        self.incomplete_buffer.push_str(&buf);
                    }
                }
                Err(e) => {
                    warn!(
                        path = %self.path.display(),
                        error = %e,
                        "error reading JSONL file"
                    );
                    break;
                }
            }
        }

        // Update seek position
        if let Ok(pos) = reader.stream_position() {
            self.seek_pos = pos;
        }

        lines
    }

    /// Get the current file path being watched.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Scan JSONL file from byte 0 to `end_pos`, backfilling title state.
///
/// - Finds the LAST `custom-title` → `last_title` (newest explicit title wins)
/// - Finds the LAST `summary` → `last_summary` (newest AI summary wins)
/// - Finds the FIRST `user` message text → `last_first_prompt` (first message as fallback)
///
/// Does not change the watcher's `seek_pos`; only populates the title fields.
fn scan_historical(
    path: &Path,
    end_pos: u64,
    last_title: &mut Option<String>,
    last_summary: &mut Option<String>,
    last_first_prompt: &mut Option<String>,
) {
    let Ok(file) = File::open(path) else { return };
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    let mut pos = 0u64;

    while pos < end_pos {
        buf.clear();
        let n = match reader.read_line(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pos += n as u64;

        let line = buf.trim_end_matches('\n').trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match v["type"].as_str() {
            Some("custom-title") => {
                if let Some(t) = v["customTitle"].as_str().filter(|s| !s.is_empty()) {
                    *last_title = Some(truncate_title(t));
                }
            }
            Some("summary") => {
                if let Some(s) = v["summary"].as_str().filter(|s| !s.is_empty()) {
                    *last_summary = Some(truncate_title(s));
                }
            }
            Some("user") if last_first_prompt.is_none() => {
                if let Some(text) = extract_user_text(&v) {
                    *last_first_prompt = Some(truncate_title(&text));
                }
            }
            _ => {}
        }
    }
}

/// Extract plain text from a Claude JSONL `user` line's message content.
///
/// Handles both array form (`content: [{type:"text", text:"..."}]`)
/// and string form (`content: "..."`).
pub(crate) fn extract_user_text(v: &serde_json::Value) -> Option<String> {
    let content = &v["message"]["content"];
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item["type"].as_str() == Some("text")
                && let Some(text) = item["text"].as_str()
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    if let Some(s) = content.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Truncate a title string to `MAX_TITLE_LEN` characters, appending "…" if cut.
fn truncate_title(s: &str) -> String {
    if s.len() <= MAX_TITLE_LEN {
        s.to_string()
    } else {
        // Truncate at a char boundary
        let boundary = s
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= MAX_TITLE_LEN - 3)
            .last()
            .unwrap_or(0);
        format!("{}…", &s[..boundary])
    }
}

/// Get file size and inode for rotation detection.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_jsonl(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("agtmux-test-watcher");
        fs::create_dir_all(&dir).expect("test");
        dir.join(name)
    }

    #[test]
    fn watcher_reads_new_lines() {
        let path = temp_jsonl("test-read-lines.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = SessionFileWatcher::new_from_start(path.clone());

        // Write some lines
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(f, r#"{{"type":"user","timestamp":"2026-02-25T13:00:00Z"}}"#).expect("test");
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-02-25T13:00:01Z"}}"#
        )
        .expect("test");

        let lines = watcher.poll_new_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("user"));
        assert!(lines[1].contains("assistant"));

        // Second poll — no new lines
        let lines2 = watcher.poll_new_lines();
        assert!(lines2.is_empty());

        // Write more
        writeln!(
            f,
            r#"{{"type":"tool_use","timestamp":"2026-02-25T13:00:02Z"}}"#
        )
        .expect("test");
        let lines3 = watcher.poll_new_lines();
        assert_eq!(lines3.len(), 1);
        assert!(lines3[0].contains("tool_use"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_skips_partial_line() {
        let path = temp_jsonl("test-partial-line.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = SessionFileWatcher::new_from_start(path.clone());

        // Write a partial line (no trailing newline)
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        write!(f, r#"{{"type":"user","timesta"#).expect("test");

        let lines = watcher.poll_new_lines();
        assert!(lines.is_empty(), "partial line should not be returned");

        // Complete the line
        writeln!(f, r#"mp":"2026-02-25T13:00:00Z"}}"#).expect("test");

        let lines2 = watcher.poll_new_lines();
        assert_eq!(lines2.len(), 1);
        assert!(lines2[0].contains("user"));
        assert!(lines2[0].contains("2026-02-25T13:00:00Z"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn watcher_handles_file_rotation() {
        let path = temp_jsonl("test-rotation.jsonl");
        fs::write(&path, "").expect("test");

        let mut watcher = SessionFileWatcher::new_from_start(path.clone());

        // Write and read
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(f, r#"{{"type":"user"}}"#).expect("test");
        drop(f);
        let lines = watcher.poll_new_lines();
        assert_eq!(lines.len(), 1);

        // Simulate rotation: delete and recreate the file
        fs::remove_file(&path).expect("test");
        fs::write(&path, "").expect("test");
        let mut f2 = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(f2, r#"{{"type":"assistant"}}"#).expect("test");
        drop(f2);

        let lines2 = watcher.poll_new_lines();
        // After rotation, should read from the new file
        assert!(!lines2.is_empty());
        assert!(lines2[0].contains("assistant"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn new_watcher_seeks_to_eof() {
        let path = temp_jsonl("test-eof-seek.jsonl");
        // Write historical data
        fs::write(
            &path,
            r#"{"type":"user","timestamp":"2026-02-25T10:00:00Z"}
{"type":"assistant","timestamp":"2026-02-25T10:00:01Z"}
"#,
        )
        .expect("test");

        // New watcher should skip existing content
        let mut watcher = SessionFileWatcher::new(path.clone());
        let lines = watcher.poll_new_lines();
        assert!(lines.is_empty(), "should skip historical data");

        // New lines should be picked up
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("test");
        writeln!(
            f,
            r#"{{"type":"tool_use","timestamp":"2026-02-25T14:00:00Z"}}"#
        )
        .expect("test");

        let lines2 = watcher.poll_new_lines();
        assert_eq!(lines2.len(), 1);
        assert!(lines2[0].contains("tool_use"));

        let _ = fs::remove_file(&path);
    }
}
