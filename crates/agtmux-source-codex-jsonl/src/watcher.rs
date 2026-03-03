//! File watcher for Codex JSONL session files.
//!
//! Tracks seek position per session file, handles partial lines,
//! and detects file rotation via inode changes.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tracing::warn;

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
}

impl CodexSessionFileWatcher {
    /// Create a new watcher, seeking to EOF so that future lines are tracked.
    pub fn new(path: PathBuf) -> Self {
        let (seek_pos, inode) = file_metadata(&path).unwrap_or((0, 0));
        Self {
            path,
            seek_pos,
            inode,
            incomplete_buffer: String::new(),
            bootstrapped: false,
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
