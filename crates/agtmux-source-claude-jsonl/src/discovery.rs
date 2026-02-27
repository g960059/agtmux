//! CWD-based session discovery for Claude Code JSONL files.
//!
//! Maps a tmux pane's current working directory to the corresponding
//! Claude Code JSONL transcript file by reading `sessions-index.json`.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Entry in the `sessions-index.json` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub full_path: String,
    pub project_path: String,
    #[serde(default)]
    pub git_branch: Option<String>,
    pub modified: DateTime<Utc>,
    #[serde(default)]
    pub is_sidechain: bool,
}

/// The `sessions-index.json` file structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsIndex {
    #[allow(dead_code)]
    pub version: u32,
    pub original_path: String,
    pub entries: Vec<SessionIndexEntry>,
}

/// Result of a successful session discovery for a pane.
#[derive(Debug, Clone)]
pub struct SessionDiscovery {
    pub pane_id: String,
    pub session_id: String,
    pub jsonl_path: PathBuf,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<DateTime<Utc>>,
}

/// Encode a path to the format used by Claude Code for project directories.
/// Example: `/Users/vm/project` -> `-Users-vm-project`
pub fn encode_path(path: &str) -> String {
    path.replace('/', "-")
}

/// Discover JSONL session files for the given pane CWDs.
///
/// `pane_cwds` is a list of `(pane_id, canonical_cwd, pane_generation, pane_birth_ts)`.
#[allow(clippy::type_complexity)]
pub fn discover_sessions(
    pane_cwds: &[(String, String, Option<u64>, Option<DateTime<Utc>>)],
) -> Vec<SessionDiscovery> {
    let claude_dir = match home_dir() {
        Some(home) => home.join(".claude").join("projects"),
        None => {
            warn!("could not determine home directory for Claude JSONL discovery");
            return Vec::new();
        }
    };

    let mut results = Vec::new();

    for (pane_id, cwd, pane_gen, pane_birth) in pane_cwds {
        // Canonicalize the CWD to resolve symlinks/worktrees
        let canonical_cwd = match std::fs::canonicalize(cwd) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => cwd.clone(),
        };

        let encoded = encode_path(&canonical_cwd);
        let index_path = claude_dir.join(&encoded).join("sessions-index.json");

        if !index_path.exists() {
            continue;
        }

        match read_sessions_index(&index_path) {
            Ok(index) => {
                if let Some(entry) = find_best_session(&index, &canonical_cwd) {
                    let jsonl_path = PathBuf::from(&entry.full_path);
                    if jsonl_path.exists() {
                        results.push(SessionDiscovery {
                            pane_id: pane_id.clone(),
                            session_id: entry.session_id.clone(),
                            jsonl_path,
                            pane_generation: *pane_gen,
                            pane_birth_ts: *pane_birth,
                        });
                    }
                }
            }
            Err(e) => {
                warn!(
                    path = %index_path.display(),
                    error = %e,
                    "failed to read sessions-index.json"
                );
            }
        }
    }

    results
}

/// Read and parse a `sessions-index.json` file.
fn read_sessions_index(path: &Path) -> Result<SessionsIndex, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("parse error: {e}"))
}

/// Find the best (latest, non-sidechain) session entry matching the CWD.
fn find_best_session<'a>(
    index: &'a SessionsIndex,
    canonical_cwd: &str,
) -> Option<&'a SessionIndexEntry> {
    // Check originalPath match first (fast path)
    let cwd_matches_original = normalize_path_for_compare(&index.original_path)
        == normalize_path_for_compare(canonical_cwd);

    index
        .entries
        .iter()
        .filter(|e| {
            !e.is_sidechain
                && (cwd_matches_original
                    || normalize_path_for_compare(&e.project_path)
                        == normalize_path_for_compare(canonical_cwd))
        })
        .max_by_key(|e| e.modified)
}

/// Normalize a path for comparison (strip trailing slash).
fn normalize_path_for_compare(path: &str) -> &str {
    path.strip_suffix('/').unwrap_or(path)
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn encode_path_replaces_slashes() {
        assert_eq!(encode_path("/Users/vm/project"), "-Users-vm-project");
        assert_eq!(encode_path("/"), "-");
        assert_eq!(encode_path("relative/path"), "relative-path");
    }

    #[test]
    fn normalize_path_strips_trailing_slash() {
        assert_eq!(
            normalize_path_for_compare("/Users/vm/project/"),
            "/Users/vm/project"
        );
        assert_eq!(
            normalize_path_for_compare("/Users/vm/project"),
            "/Users/vm/project"
        );
    }

    #[test]
    fn find_best_session_picks_latest_non_sidechain() {
        let index = SessionsIndex {
            version: 1,
            original_path: "/Users/vm/project".to_owned(),
            entries: vec![
                SessionIndexEntry {
                    session_id: "old-session".to_owned(),
                    full_path: "/tmp/old.jsonl".to_owned(),
                    project_path: "/Users/vm/project".to_owned(),
                    git_branch: None,
                    modified: "2026-02-25T10:00:00Z".parse().expect("test"),
                    is_sidechain: false,
                },
                SessionIndexEntry {
                    session_id: "new-session".to_owned(),
                    full_path: "/tmp/new.jsonl".to_owned(),
                    project_path: "/Users/vm/project".to_owned(),
                    git_branch: None,
                    modified: "2026-02-25T14:00:00Z".parse().expect("test"),
                    is_sidechain: false,
                },
                SessionIndexEntry {
                    session_id: "sidechain-session".to_owned(),
                    full_path: "/tmp/sidechain.jsonl".to_owned(),
                    project_path: "/Users/vm/project".to_owned(),
                    git_branch: None,
                    modified: "2026-02-25T15:00:00Z".parse().expect("test"),
                    is_sidechain: true,
                },
            ],
        };

        let result = find_best_session(&index, "/Users/vm/project");
        assert!(result.is_some());
        assert_eq!(result.expect("test").session_id, "new-session");
    }

    #[test]
    fn find_best_session_no_match() {
        let index = SessionsIndex {
            version: 1,
            original_path: "/Users/vm/other-project".to_owned(),
            entries: vec![SessionIndexEntry {
                session_id: "sess-1".to_owned(),
                full_path: "/tmp/sess.jsonl".to_owned(),
                project_path: "/Users/vm/other-project".to_owned(),
                git_branch: None,
                modified: "2026-02-25T10:00:00Z".parse().expect("test"),
                is_sidechain: false,
            }],
        };

        let result = find_best_session(&index, "/Users/vm/different-project");
        assert!(result.is_none());
    }

    #[test]
    fn read_sessions_index_roundtrip() {
        let tmp = std::env::temp_dir().join("agtmux-test-jsonl-index-rt");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let index_json = serde_json::json!({
            "version": 1,
            "originalPath": "/Users/vm/project",
            "entries": [{
                "sessionId": "sess-abc",
                "fullPath": "/tmp/sess-abc.jsonl",
                "projectPath": "/Users/vm/project",
                "modified": "2026-02-25T12:00:00Z",
                "isSidechain": false
            }]
        });
        let path = tmp.join("sessions-index.json");
        fs::write(&path, serde_json::to_string(&index_json).expect("test")).expect("test");

        let index = read_sessions_index(&path).expect("test");
        assert_eq!(index.original_path, "/Users/vm/project");
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].session_id, "sess-abc");

        let _ = fs::remove_dir_all(&tmp);
    }
}
