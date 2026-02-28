//! File watcher for Claude JSONL transcript files.
//!
//! Tracks seek position per session file, handles partial lines,
//! and detects file rotation via inode changes.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tracing::warn;

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
}

impl SessionFileWatcher {
    /// Create a new watcher for the given JSONL file path.
    ///
    /// On first creation, seeks to EOF (skips historical data).
    pub fn new(path: PathBuf) -> Self {
        let (seek_pos, inode) = match file_metadata(&path) {
            Some((size, ino)) => (size, ino),
            None => (0, 0),
        };

        Self {
            path,
            seek_pos,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
            last_title: None,
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
