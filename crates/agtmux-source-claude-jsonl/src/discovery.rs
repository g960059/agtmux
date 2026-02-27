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

/// Discover JSONL session files for the given pane CWDs.
///
/// `pane_cwds` is a list of `(pane_id, canonical_cwd, pane_generation, pane_birth_ts)`.
#[allow(clippy::type_complexity)]
pub fn discover_sessions(
    pane_cwds: &[(String, String, Option<u64>, Option<DateTime<Utc>>)],
) -> Vec<SessionDiscovery> {
    let claude_projects_dir = match home_dir() {
        Some(home) => home.join(".claude").join("projects"),
        None => {
            warn!("could not determine home directory for Claude JSONL discovery");
            return Vec::new();
        }
    };

    discover_sessions_in_projects_dir(pane_cwds, &claude_projects_dir)
}

#[allow(clippy::type_complexity)]
fn discover_sessions_in_projects_dir(
    pane_cwds: &[(String, String, Option<u64>, Option<DateTime<Utc>>)],
    claude_projects_dir: &Path,
) -> Vec<SessionDiscovery> {
    // Resolve canonical CWD once per pane to avoid redundant filesystem calls.
    let canonical_cwds: Vec<String> = pane_cwds
        .iter()
        .map(|(_, cwd, _, _)| {
            std::fs::canonicalize(cwd)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| cwd.clone())
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

    for ((pane_id, _cwd, pane_gen, pane_birth), canonical_cwd) in
        pane_cwds.iter().zip(&canonical_cwds)
    {
        let cwd_candidate_count = *cwd_count.get(canonical_cwd.as_str()).unwrap_or(&1);
        let project_dir = claude_projects_dir.join(encode_path(canonical_cwd));
        let Some((session_id, jsonl_path)) = discover_project_session(&project_dir, canonical_cwd)
        else {
            continue;
        };

        results.push(SessionDiscovery {
            pane_id: pane_id.clone(),
            session_id,
            jsonl_path,
            pane_generation: *pane_gen,
            pane_birth_ts: *pane_birth,
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

fn is_jsonl_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

fn session_id_from_jsonl_path(path: &Path) -> String {
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

        let pane_cwds = vec![("%42".to_owned(), canonical_cwd, Some(9), None)];
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

        let pane_cwds = vec![("%11".to_owned(), canonical_cwd, None, None)];
        let discoveries = discover_sessions_in_projects_dir(&pane_cwds, &claude_projects_dir);

        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].pane_id, "%11");
        assert_eq!(discoveries[0].session_id, "fallback-session");
        assert_eq!(discoveries[0].jsonl_path, jsonl_path);

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
            ("%1".to_owned(), shared_canonical.clone(), None, None),
            ("%2".to_owned(), shared_canonical.clone(), None, None),
            ("%3".to_owned(), solo_canonical.clone(), None, None),
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
}
