//! CWD-based session discovery for Claude Code JSONL files.
//!
//! Maps a tmux pane's current working directory to the corresponding
//! Claude Code JSONL transcript file by checking `sessions-index.json`
//! and falling back to project-directory `*.jsonl` scan.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::warn;

/// Input hint for a candidate pane during JSONL session discovery.
#[derive(Debug, Clone)]
pub struct PaneDiscoveryHint {
    pub pane_id: String,
    pub cwd: String,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
    /// PID of the process running in this pane (tmux `#{pane_pid}`).
    /// Used for fd-based P2 discovery via lsof.
    pub pane_pid: Option<u32>,
}

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
    // T-135c: title fallback fields
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub first_prompt: Option<String>,
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
    /// Number of candidate panes that share the same canonical CWD.
    ///
    /// Used by `poll_files` to choose between a full bootstrap event
    /// (`is_heartbeat=false`, count == 1) and an ambiguous bootstrap
    /// (`is_heartbeat=true`, count > 1).  When multiple panes share a CWD
    /// — e.g. Codex and Claude running side-by-side in the same project —
    /// emitting a full bootstrap for every pane would poison
    /// `last_real_activity[Claude]` for Codex panes and cause false
    /// Claude attribution.  The ambiguous bootstrap only refreshes
    /// `deterministic_last_seen`, leaving `last_real_activity` untouched
    /// so `select_winning_provider` can resolve the conflict via actual
    /// activity timestamps rather than bootstrap timing.
    pub cwd_candidate_count: usize,
}

/// Encode a path to the format used by Claude Code for project directories.
/// Example: `/Users/vm/project` -> `-Users-vm-project`
pub fn encode_path(path: &str) -> String {
    path.chars()
        .map(|ch| if ch == '/' || ch == '.' { '-' } else { ch })
        .collect()
}

/// Discover JSONL session files for the given pane hints.
pub fn discover_sessions(hints: &[PaneDiscoveryHint]) -> Vec<SessionDiscovery> {
    let claude_projects_dir = match home_dir() {
        Some(home) => home.join(".claude").join("projects"),
        None => {
            warn!("could not determine home directory for Claude JSONL discovery");
            return Vec::new();
        }
    };

    discover_sessions_in_projects_dir(hints, &claude_projects_dir)
}

fn discover_sessions_in_projects_dir(
    hints: &[PaneDiscoveryHint],
    claude_projects_dir: &Path,
) -> Vec<SessionDiscovery> {
    // Resolve canonical CWD once per pane to avoid redundant filesystem calls.
    let canonical_cwds: Vec<String> = hints
        .iter()
        .map(|hint| {
            std::fs::canonicalize(&hint.cwd)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| hint.cwd.clone())
        })
        .collect();

    // Count how many candidate panes share each canonical CWD.
    // Used to distinguish single-pane vs. multi-pane CWD scenarios for
    // bootstrap event selection (full bootstrap vs. ambiguous bootstrap).
    let mut cwd_count: HashMap<&str, usize> = HashMap::new();
    for c in &canonical_cwds {
        *cwd_count.entry(c.as_str()).or_insert(0) += 1;
    }

    let mut results = Vec::new();

    for (hint, canonical_cwd) in hints.iter().zip(&canonical_cwds) {
        let cwd_candidate_count = *cwd_count.get(canonical_cwd.as_str()).unwrap_or(&1);

        // P2: fd-based discovery (lsof) — higher confidence than CWD-based
        if let Some(pane_pid) = hint.pane_pid
            && let Some((session_id, jsonl_path)) =
                discover_jsonl_via_lsof(pane_pid, claude_projects_dir)
        {
            results.push(SessionDiscovery {
                pane_id: hint.pane_id.clone(),
                session_id,
                jsonl_path,
                pane_generation: hint.pane_generation,
                pane_birth_ts: hint.pane_birth_ts,
                cwd_candidate_count: 1, // fd-based is unambiguous
            });
            continue; // skip P3 for this pane
        }

        // P3: CWD-based discovery (existing fallback)
        let project_dir = claude_projects_dir.join(encode_path(canonical_cwd));
        let Some((session_id, jsonl_path)) = discover_project_session(&project_dir, canonical_cwd)
        else {
            continue;
        };

        results.push(SessionDiscovery {
            pane_id: hint.pane_id.clone(),
            session_id,
            jsonl_path,
            pane_generation: hint.pane_generation,
            pane_birth_ts: hint.pane_birth_ts,
            cwd_candidate_count,
        });
    }

    results
}

fn discover_project_session(project_dir: &Path, canonical_cwd: &str) -> Option<(String, PathBuf)> {
    if !project_dir.is_dir() {
        return None;
    }

    // Prefer sessions-index when present for sidechain filtering and explicit mapping.
    if let Some(from_index) = discover_from_sessions_index(project_dir, canonical_cwd) {
        return Some(from_index);
    }

    discover_latest_jsonl_file(project_dir)
}

fn discover_from_sessions_index(
    project_dir: &Path,
    canonical_cwd: &str,
) -> Option<(String, PathBuf)> {
    let index_path = project_dir.join("sessions-index.json");
    if !index_path.exists() {
        return None;
    }

    match read_sessions_index(&index_path) {
        Ok(index) => {
            if let Some(entry) = find_best_session(&index, canonical_cwd) {
                let jsonl_path = PathBuf::from(&entry.full_path);
                if jsonl_path.exists() {
                    return Some((entry.session_id.clone(), jsonl_path));
                }
                warn!(
                    path = %jsonl_path.display(),
                    session_id = %entry.session_id,
                    "sessions-index entry points to missing JSONL file; falling back to project scan"
                );
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

    None
}

fn discover_latest_jsonl_file(project_dir: &Path) -> Option<(String, PathBuf)> {
    let entries = match std::fs::read_dir(project_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(
                path = %project_dir.display(),
                error = %e,
                "failed to read Claude project directory for JSONL fallback discovery"
            );
            return None;
        }
    };

    let mut best: Option<(PathBuf, SystemTime)> = None;

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!(
                    path = %project_dir.display(),
                    error = %e,
                    "failed to read directory entry during JSONL fallback discovery"
                );
                continue;
            }
        };

        let path = entry.path();
        if !is_jsonl_file(&path) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to read JSONL file metadata during fallback discovery"
                );
                continue;
            }
        };

        if !metadata.is_file() {
            continue;
        }

        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        let is_better = match &best {
            None => true,
            Some((best_path, best_modified)) => {
                modified > *best_modified
                    || (modified == *best_modified
                        && path.to_string_lossy() > best_path.to_string_lossy())
            }
        };

        if is_better {
            best = Some((path, modified));
        }
    }

    best.map(|(path, _)| (session_id_from_jsonl_path(&path), path))
}

/// Attempt to find an open Claude Code JSONL file for a given PID using `lsof`.
///
/// Returns the first matching `(session_id, jsonl_path)` found.
/// Returns `None` if lsof is not available, fails, or finds no JSONL files.
fn discover_jsonl_via_lsof(pid: u32, claude_projects_dir: &Path) -> Option<(String, PathBuf)> {
    use std::process::Command;
    use std::time::SystemTime;

    let output = Command::new("lsof")
        .args(["-F", "n", "-p", &pid.to_string()])
        .output()
        .ok()?;

    // lsof returns non-zero if the pid has no open files matching, but stdout may still have data.
    // We proceed with whatever stdout we got.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let projects_prefix = claude_projects_dir.to_string_lossy();

    let mut best: Option<(PathBuf, SystemTime)> = None;

    for line in stdout.lines() {
        // lsof -F n output: lines starting with 'n' are file names
        let Some(path_str) = line.strip_prefix('n') else {
            continue;
        };
        let path = Path::new(path_str);
        if !is_jsonl_file(path) {
            continue;
        }
        // Must be inside claude_projects_dir
        if !path_str.starts_with(projects_prefix.as_ref()) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(path) {
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let is_better = match &best {
                None => true,
                Some((_, best_modified)) => modified > *best_modified,
            };
            if is_better {
                best = Some((path.to_path_buf(), modified));
            }
        }
    }

    best.map(|(path, _)| (session_id_from_jsonl_path(&path), path))
}

fn is_jsonl_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

pub fn session_id_from_jsonl_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "unknown-session".to_owned())
}

/// Create a [`SessionDiscovery`] directly from a transcript path provided by a SessionStart hook.
/// Returns `None` if the file does not exist.
pub fn discovery_from_transcript_path(
    pane_id: &str,
    jsonl_path: &std::path::Path,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<SessionDiscovery> {
    if !jsonl_path.exists() {
        return None;
    }
    Some(SessionDiscovery {
        pane_id: pane_id.to_owned(),
        session_id: session_id_from_jsonl_path(jsonl_path),
        jsonl_path: jsonl_path.to_path_buf(),
        pane_generation,
        pane_birth_ts,
        cwd_candidate_count: 1,
    })
}

/// Read and parse a `sessions-index.json` file.
fn read_sessions_index(path: &Path) -> Result<SessionsIndex, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("parse error: {e}"))
}

/// Look up a session entry by `session_id` from the `sessions-index.json`
/// in the given project directory.
///
/// Returns `None` if the file doesn't exist, can't be parsed, or the
/// `session_id` isn't found.  This is a best-effort fallback — callers
/// must handle `None` gracefully.
pub fn read_session_index_entry(project_dir: &Path, session_id: &str) -> Option<SessionIndexEntry> {
    let path = project_dir.join("sessions-index.json");
    let index = read_sessions_index(&path).ok()?;
    index
        .entries
        .into_iter()
        .find(|e| e.session_id == session_id)
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
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn encode_path_replaces_slashes_and_dots() {
        assert_eq!(encode_path("/Users/vm/project"), "-Users-vm-project");
        assert_eq!(
            encode_path("/Users/virtualmachine/ghq/github.com/g960059/repo"),
            "-Users-virtualmachine-ghq-github-com-g960059-repo"
        );
        assert_eq!(encode_path("/"), "-");
        assert_eq!(encode_path("relative/path"), "relative-path");
        assert_eq!(encode_path("github.com/repo"), "github-com-repo");
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
                    summary: None,
                    first_prompt: None,
                },
                SessionIndexEntry {
                    session_id: "new-session".to_owned(),
                    full_path: "/tmp/new.jsonl".to_owned(),
                    project_path: "/Users/vm/project".to_owned(),
                    git_branch: None,
                    modified: "2026-02-25T14:00:00Z".parse().expect("test"),
                    is_sidechain: false,
                    summary: None,
                    first_prompt: None,
                },
                SessionIndexEntry {
                    session_id: "sidechain-session".to_owned(),
                    full_path: "/tmp/sidechain.jsonl".to_owned(),
                    project_path: "/Users/vm/project".to_owned(),
                    git_branch: None,
                    modified: "2026-02-25T15:00:00Z".parse().expect("test"),
                    is_sidechain: true,
                    summary: None,
                    first_prompt: None,
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
                summary: None,
                first_prompt: None,
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

    #[test]
    fn session_index_entry_parses_summary_and_first_prompt() {
        let entry_json = serde_json::json!({
            "sessionId": "test-sess",
            "fullPath": "/tmp/test-sess.jsonl",
            "projectPath": "/Users/vm/project",
            "modified": "2026-02-25T12:00:00Z",
            "isSidechain": false,
            "summary": "Test Summary",
            "firstPrompt": "How do I fix this bug?"
        });
        let entry: SessionIndexEntry = serde_json::from_value(entry_json).expect("should parse");
        assert_eq!(entry.session_id, "test-sess");
        assert_eq!(entry.summary, Some("Test Summary".to_owned()));
        assert_eq!(
            entry.first_prompt,
            Some("How do I fix this bug?".to_owned())
        );
    }

    #[test]
    fn read_session_index_entry_finds_by_session_id() {
        let tmp = std::env::temp_dir().join("agtmux-test-read-index-entry");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("test");

        let index_json = serde_json::json!({
            "version": 1,
            "originalPath": "/Users/vm/project",
            "entries": [
                {
                    "sessionId": "sess-a",
                    "fullPath": "/tmp/sess-a.jsonl",
                    "projectPath": "/Users/vm/project",
                    "modified": "2026-02-25T10:00:00Z",
                    "isSidechain": false,
                    "summary": "Summary A",
                    "firstPrompt": "Prompt A"
                },
                {
                    "sessionId": "sess-b",
                    "fullPath": "/tmp/sess-b.jsonl",
                    "projectPath": "/Users/vm/project",
                    "modified": "2026-02-25T11:00:00Z",
                    "isSidechain": false
                }
            ]
        });
        let index_path = tmp.join("sessions-index.json");
        fs::write(
            &index_path,
            serde_json::to_string(&index_json).expect("test"),
        )
        .expect("test");

        let entry_a = read_session_index_entry(&tmp, "sess-a");
        assert!(entry_a.is_some(), "sess-a should be found");
        let entry_a = entry_a.expect("test");
        assert_eq!(entry_a.session_id, "sess-a");
        assert_eq!(entry_a.summary, Some("Summary A".to_owned()));
        assert_eq!(entry_a.first_prompt, Some("Prompt A".to_owned()));

        let entry_b = read_session_index_entry(&tmp, "sess-b");
        assert!(entry_b.is_some(), "sess-b should be found");
        assert_eq!(entry_b.expect("test").session_id, "sess-b");

        let entry_none = read_session_index_entry(&tmp, "nonexistent");
        assert!(entry_none.is_none(), "nonexistent should return None");

        let _ = fs::remove_dir_all(&tmp);
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-test-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("test");
        dir
    }

    #[test]
    fn discover_sessions_falls_back_to_latest_jsonl_without_index() {
        let tmp = unique_temp_dir("discover-fallback-no-index");
        let claude_projects_dir = tmp.join("claude-projects");
        let cwd = tmp.join("workspace");
        fs::create_dir_all(&cwd).expect("test");
        let canonical_cwd = fs::canonicalize(&cwd).expect("test");
        let canonical_cwd = canonical_cwd.to_string_lossy().to_string();

        let project_dir = claude_projects_dir.join(encode_path(&canonical_cwd));
        fs::create_dir_all(&project_dir).expect("test");

        let old_jsonl = project_dir.join("a-old-session.jsonl");
        let new_jsonl = project_dir.join("z-new-session.jsonl");
        fs::write(&old_jsonl, "{}\n").expect("test");
        std::thread::sleep(Duration::from_millis(10));
        fs::write(&new_jsonl, "{}\n").expect("test");
        fs::write(project_dir.join("README.txt"), "ignore").expect("test");

        let pane_cwds = vec![PaneDiscoveryHint {
            pane_id: "%42".to_owned(),
            cwd: canonical_cwd,
            pane_generation: Some(9),
            pane_birth_ts: None,
            pane_pid: None,
        }];
        let discoveries = discover_sessions_in_projects_dir(&pane_cwds, &claude_projects_dir);

        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].pane_id, "%42");
        assert_eq!(discoveries[0].session_id, "z-new-session");
        assert_eq!(discoveries[0].jsonl_path, new_jsonl);
        assert_eq!(discoveries[0].pane_generation, Some(9));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_falls_back_when_index_is_invalid() {
        let tmp = unique_temp_dir("discover-fallback-invalid-index");
        let claude_projects_dir = tmp.join("claude-projects");
        let cwd = tmp.join("workspace");
        fs::create_dir_all(&cwd).expect("test");
        let canonical_cwd = fs::canonicalize(&cwd).expect("test");
        let canonical_cwd = canonical_cwd.to_string_lossy().to_string();

        let project_dir = claude_projects_dir.join(encode_path(&canonical_cwd));
        fs::create_dir_all(&project_dir).expect("test");
        fs::write(project_dir.join("sessions-index.json"), "{not-valid-json").expect("test");

        let jsonl_path = project_dir.join("fallback-session.jsonl");
        fs::write(&jsonl_path, "{}\n").expect("test");

        let pane_cwds = vec![PaneDiscoveryHint {
            pane_id: "%11".to_owned(),
            cwd: canonical_cwd,
            pane_generation: None,
            pane_birth_ts: None,
            pane_pid: None,
        }];
        let discoveries = discover_sessions_in_projects_dir(&pane_cwds, &claude_projects_dir);

        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].pane_id, "%11");
        assert_eq!(discoveries[0].session_id, "fallback-session");
        assert_eq!(discoveries[0].jsonl_path, jsonl_path);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discovery_from_transcript_path_returns_none_when_file_missing() {
        let nonexistent = std::path::Path::new("/tmp/agtmux-nonexistent-session-abc.jsonl");
        let result = discovery_from_transcript_path("%5", nonexistent, Some(3), None);
        assert!(result.is_none());
    }

    #[test]
    fn discovery_from_transcript_path_returns_some_when_file_exists() {
        let tmp = unique_temp_dir("transcript-hint");
        let jsonl_path = tmp.join("my-session-id.jsonl");
        fs::write(&jsonl_path, "{}\n").expect("test");

        let result = discovery_from_transcript_path("%7", &jsonl_path, Some(2), None);
        assert!(result.is_some());
        let disc = result.expect("test");
        assert_eq!(disc.pane_id, "%7");
        assert_eq!(disc.session_id, "my-session-id");
        assert_eq!(disc.jsonl_path, jsonl_path);
        assert_eq!(disc.pane_generation, Some(2));
        assert!(disc.pane_birth_ts.is_none());
        assert_eq!(disc.cwd_candidate_count, 1);

        let _ = fs::remove_dir_all(&tmp);
    }

    /// When two panes share the same CWD (e.g. Codex + Claude in the same project),
    /// `cwd_candidate_count` must be 2 for both discoveries.
    /// A single-pane CWD must have count == 1.
    #[test]
    fn discover_sessions_cwd_candidate_count_multi_pane() {
        let tmp = unique_temp_dir("discover-cwd-count");
        let claude_projects_dir = tmp.join("claude-projects");

        // Two panes share the same CWD.
        let shared_cwd = tmp.join("shared-workspace");
        fs::create_dir_all(&shared_cwd).expect("test");
        let shared_canonical = fs::canonicalize(&shared_cwd)
            .expect("test")
            .to_string_lossy()
            .to_string();

        // A third pane has a different CWD.
        let solo_cwd = tmp.join("solo-workspace");
        fs::create_dir_all(&solo_cwd).expect("test");
        let solo_canonical = fs::canonicalize(&solo_cwd)
            .expect("test")
            .to_string_lossy()
            .to_string();

        // Create JSONL files for both CWDs.
        for cwd_str in [&shared_canonical, &solo_canonical] {
            let dir = claude_projects_dir.join(encode_path(cwd_str));
            fs::create_dir_all(&dir).expect("test");
            fs::write(dir.join("session.jsonl"), "{}\n").expect("test");
        }

        let pane_cwds = vec![
            PaneDiscoveryHint {
                pane_id: "%1".to_owned(),
                cwd: shared_canonical.clone(),
                pane_generation: None,
                pane_birth_ts: None,
                pane_pid: None,
            },
            PaneDiscoveryHint {
                pane_id: "%2".to_owned(),
                cwd: shared_canonical.clone(),
                pane_generation: None,
                pane_birth_ts: None,
                pane_pid: None,
            },
            PaneDiscoveryHint {
                pane_id: "%3".to_owned(),
                cwd: solo_canonical.clone(),
                pane_generation: None,
                pane_birth_ts: None,
                pane_pid: None,
            },
        ];
        let discoveries = discover_sessions_in_projects_dir(&pane_cwds, &claude_projects_dir);

        assert_eq!(discoveries.len(), 3);

        // Both shared-CWD panes should report count == 2.
        let d1 = discoveries.iter().find(|d| d.pane_id == "%1").expect("d1");
        let d2 = discoveries.iter().find(|d| d.pane_id == "%2").expect("d2");
        let d3 = discoveries.iter().find(|d| d.pane_id == "%3").expect("d3");

        assert_eq!(d1.cwd_candidate_count, 2, "%1 shares CWD with %2 → count 2");
        assert_eq!(d2.cwd_candidate_count, 2, "%2 shares CWD with %1 → count 2");
        assert_eq!(d3.cwd_candidate_count, 1, "%3 has unique CWD → count 1");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_jsonl_via_lsof_nonexistent_pid_returns_none() {
        let tmp = std::env::temp_dir();
        // PID 999999999 is almost certainly invalid → lsof returns error or empty
        let result = discover_jsonl_via_lsof(999999999, &tmp);
        assert!(result.is_none());
    }
}
