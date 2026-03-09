//! CWD-based session discovery for Codex JSONL files.
//!
//! Uses lsof to get the pane's CWD (timing-independent via cwd fd),
//! then scans ~/.codex/sessions/**/*.jsonl matching by session_meta CWD.
//!
//! When multiple panes share the same CWD, discovery must assign distinct JSONL
//! sessions per pane instead of letting every pane claim the newest transcript.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use tracing::warn;

/// Input hint for a Codex pane.
#[derive(Debug, Clone)]
pub struct CodexPaneHint {
    pub pane_id: String,
    /// PID of the process in the pane (for lsof CWD lookup).
    pub pane_pid: Option<u32>,
    /// Fallback CWD from tmux (used if lsof fails).
    pub cwd: String,
    /// Previously assigned JSONL path for this pane, if any.
    /// Discovery keeps this ownership stable while the file still matches.
    pub existing_jsonl_path: Option<PathBuf>,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
}

/// Result of a successful Codex session discovery for a pane.
#[derive(Debug, Clone)]
pub struct CodexSessionDiscovery {
    pub pane_id: String,
    /// Session key derived from the JSONL filename.
    pub session_key: String,
    pub jsonl_path: PathBuf,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<chrono::DateTime<chrono::Utc>>,
}

/// Top-level entry point: discover Codex JSONL sessions for given pane hints.
pub fn discover_sessions(hints: &[CodexPaneHint]) -> Vec<CodexSessionDiscovery> {
    let codex_sessions_dir = match home_dir() {
        Some(home) => home.join(".codex").join("sessions"),
        None => {
            warn!("could not determine home directory for Codex JSONL discovery");
            return Vec::new();
        }
    };
    discover_sessions_in_dir(hints, &codex_sessions_dir)
}

/// Testable inner function that takes an explicit sessions directory.
pub fn discover_sessions_in_dir(
    hints: &[CodexPaneHint],
    codex_sessions_dir: &Path,
) -> Vec<CodexSessionDiscovery> {
    // Build index: canonical_cwd -> newest-first candidate sessions.
    let mut index_by_cwd: BTreeMap<String, Vec<(PathBuf, SystemTime)>> = BTreeMap::new();
    for (session_cwd, path, mtime) in build_session_index(codex_sessions_dir) {
        index_by_cwd
            .entry(canonicalize_path(&session_cwd))
            .or_default()
            .push((path, mtime));
    }
    for sessions in index_by_cwd.values_mut() {
        sessions.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }

    // Resolve pane CWDs first so multiple same-CWD panes can share a unique assignment pass.
    let mut hints_by_cwd: BTreeMap<String, Vec<&CodexPaneHint>> = BTreeMap::new();
    for hint in hints {
        let cwd = if let Some(pid) = hint.pane_pid {
            get_cwd_via_lsof(pid).unwrap_or_else(|| hint.cwd.clone())
        } else {
            hint.cwd.clone()
        };
        hints_by_cwd
            .entry(canonicalize_path(&cwd))
            .or_default()
            .push(hint);
    }

    let mut results = Vec::new();

    for (canonical_cwd, cwd_hints) in hints_by_cwd {
        let Some(candidates) = index_by_cwd.get(&canonical_cwd) else {
            continue;
        };

        let mut used_paths: HashSet<PathBuf> = HashSet::new();
        let mut pending_hints = Vec::new();

        // First preserve any live ownership so an older pane does not get rebound
        // to a sibling's newer session file on the next discovery tick.
        for hint in cwd_hints {
            let Some(existing_path) = hint.existing_jsonl_path.as_ref() else {
                pending_hints.push(hint);
                continue;
            };

            if candidates.iter().any(|(path, _)| path == existing_path)
                && used_paths.insert(existing_path.clone())
            {
                results.push(CodexSessionDiscovery {
                    pane_id: hint.pane_id.clone(),
                    session_key: session_key_from_path(existing_path),
                    jsonl_path: existing_path.clone(),
                    pane_generation: hint.pane_generation,
                    pane_birth_ts: hint.pane_birth_ts,
                });
            } else {
                pending_hints.push(hint);
            }
        }

        // Then assign remaining panes to the next-unused newest sessions.
        for hint in pending_hints {
            let Some((path, _)) = candidates
                .iter()
                .find(|(path, _)| !used_paths.contains(path))
            else {
                continue;
            };
            used_paths.insert(path.clone());
            results.push(CodexSessionDiscovery {
                pane_id: hint.pane_id.clone(),
                session_key: session_key_from_path(path),
                jsonl_path: path.clone(),
                pane_generation: hint.pane_generation,
                pane_birth_ts: hint.pane_birth_ts,
            });
        }
    }

    results
}

/// Build an index mapping `session_cwd → (jsonl_path, mtime)` by scanning all
/// date-directory JSONL files and reading their `session_meta` line 1.
fn build_session_index(sessions_dir: &Path) -> Vec<(String, PathBuf, SystemTime)> {
    let mut index = Vec::new();

    if !sessions_dir.is_dir() {
        return index;
    }

    // Walk YYYY/MM/DD subdirectory structure
    let year_dirs = match fs::read_dir(sessions_dir) {
        Ok(d) => d,
        Err(e) => {
            warn!(path = %sessions_dir.display(), error = %e, "failed to read Codex sessions dir");
            return index;
        }
    };

    for year_entry in year_dirs.flatten() {
        let year_path = year_entry.path();
        if !year_path.is_dir() {
            continue;
        }
        let month_dirs = match fs::read_dir(&year_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for month_entry in month_dirs.flatten() {
            let month_path = month_entry.path();
            if !month_path.is_dir() {
                continue;
            }
            let day_dirs = match fs::read_dir(&month_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for day_entry in day_dirs.flatten() {
                let day_path = day_entry.path();
                if !day_path.is_dir() {
                    continue;
                }
                if let Err(e) = collect_jsonl_files(&day_path, &mut index) {
                    warn!(path = %day_path.display(), error = %e, "error scanning Codex day dir");
                }
            }
        }
    }

    index
}

fn collect_jsonl_files(
    dir: &Path,
    index: &mut Vec<(String, PathBuf, SystemTime)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if let Some(cwd) = read_session_meta_cwd(&path) {
            index.push((cwd, path, mtime));
        }
    }
    Ok(())
}

/// Read the first line of a Codex JSONL file and extract `payload.cwd`.
///
/// The first line is always a `session_meta` event:
/// `{"type":"session_meta","payload":{"type":"session_meta","cwd":"/path/...","sessionId":"..."}}`
pub fn read_session_meta_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v["type"].as_str() != Some("session_meta") {
        return None;
    }
    v["payload"]["cwd"].as_str().map(ToOwned::to_owned)
}

/// Get the CWD of a process via `lsof -p <pid> -d cwd -Fn`.
///
/// The CWD file descriptor is always open (unlike regular files which may be
/// burst-written and closed), making this timing-independent.
pub fn get_cwd_via_lsof(pid: u32) -> Option<String> {
    let output = Command::new("lsof")
        .args(["-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;

    // On macOS, lsof with an invalid PID may still output all processes' CWDs.
    // We must parse the output correctly: `p<pid>` lines introduce a process block,
    // and the following `n<path>` line is that process's CWD.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid_prefix = format!("p{pid}");
    let mut in_target_pid = false;
    for line in stdout.lines() {
        if line == pid_prefix {
            in_target_pid = true;
            continue;
        }
        if line.starts_with('p') {
            // New process block — not our target
            in_target_pid = false;
        }
        if in_target_pid
            && let Some(path) = line.strip_prefix('n')
            && !path.is_empty()
        {
            return Some(path.to_owned());
        }
    }
    None
}

/// Canonicalize a path, handling macOS /tmp→/private/tmp symlink.
fn canonicalize_path(path: &str) -> String {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p.to_string_lossy().to_string();
    }
    // Fallback: manual /tmp→/private/tmp substitution (macOS)
    if path.starts_with("/tmp/") {
        return format!("/private{path}");
    }
    path.to_owned()
}

/// Derive a session key from a JSONL file path (the file stem).
fn session_key_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "unknown-codex-session".to_owned())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sessions_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-codex-disc-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("test");
        dir
    }

    fn write_session_meta(dir: &Path, filename: &str, cwd: &str) -> PathBuf {
        let path = dir.join(filename);
        let line = format!(
            r#"{{"type":"session_meta","payload":{{"type":"session_meta","cwd":"{cwd}","sessionId":"test-session"}}}}"#
        );
        fs::write(&path, format!("{line}\n")).expect("test");
        path
    }

    #[test]
    fn read_session_meta_cwd_extracts_payload_cwd() {
        let tmp = temp_sessions_dir("meta-cwd");
        let path = write_session_meta(&tmp, "session.jsonl", "/Users/vm/project");
        let cwd = read_session_meta_cwd(&path);
        assert_eq!(cwd, Some("/Users/vm/project".to_owned()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_session_meta_cwd_returns_none_for_non_session_meta() {
        let tmp = temp_sessions_dir("meta-non");
        let path = tmp.join("other.jsonl");
        fs::write(
            &path,
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
        )
        .expect("test");
        let cwd = read_session_meta_cwd(&path);
        assert!(cwd.is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_finds_matching_jsonl() {
        let tmp = temp_sessions_dir("discover-match");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        let cwd = "/Users/vm/myproject";
        write_session_meta(&day_dir, "rollout-abc.jsonl", cwd);

        let hints = vec![CodexPaneHint {
            pane_id: "%5".to_owned(),
            pane_pid: None,
            cwd: cwd.to_owned(),
            existing_jsonl_path: None,
            pane_generation: Some(1),
            pane_birth_ts: None,
        }];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].pane_id, "%5");
        assert_eq!(discoveries[0].session_key, "rollout-abc");
        assert_eq!(discoveries[0].pane_generation, Some(1));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_returns_most_recent_for_cwd() {
        let tmp = temp_sessions_dir("discover-recent");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        let cwd = "/Users/vm/project";
        // Create two sessions for the same CWD; the second one is written later
        let path1 = write_session_meta(&day_dir, "rollout-old.jsonl", cwd);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let path2 = write_session_meta(&day_dir, "rollout-new.jsonl", cwd);

        let hints = vec![CodexPaneHint {
            pane_id: "%3".to_owned(),
            pane_pid: None,
            cwd: cwd.to_owned(),
            existing_jsonl_path: None,
            pane_generation: None,
            pane_birth_ts: None,
        }];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(discoveries.len(), 1);
        // Most recently modified should be selected
        assert_eq!(
            discoveries[0].session_key, "rollout-new",
            "should pick most-recently-modified session"
        );

        let _ = fs::remove_dir_all(&tmp);
        let _ = (path1, path2); // keep alive
    }

    #[test]
    fn discover_sessions_no_match_returns_empty() {
        let tmp = temp_sessions_dir("discover-no-match");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        write_session_meta(&day_dir, "rollout-xyz.jsonl", "/Users/vm/other-project");

        let hints = vec![CodexPaneHint {
            pane_id: "%9".to_owned(),
            pane_pid: None,
            cwd: "/Users/vm/my-project".to_owned(),
            existing_jsonl_path: None,
            pane_generation: None,
            pane_birth_ts: None,
        }];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert!(discoveries.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_scans_multiple_date_dirs() {
        let tmp = temp_sessions_dir("discover-multi-date");
        let sessions_dir = tmp.join("sessions");
        // Sessions from different date directories
        let day1 = sessions_dir.join("2026").join("03").join("01");
        let day2 = sessions_dir.join("2026").join("03").join("02");
        fs::create_dir_all(&day1).expect("test");
        fs::create_dir_all(&day2).expect("test");

        let cwd = "/Users/vm/project";
        write_session_meta(&day1, "rollout-yesterday.jsonl", cwd);
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_session_meta(&day2, "rollout-today.jsonl", cwd);

        let hints = vec![CodexPaneHint {
            pane_id: "%1".to_owned(),
            pane_pid: None,
            cwd: cwd.to_owned(),
            existing_jsonl_path: None,
            pane_generation: None,
            pane_birth_ts: None,
        }];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(discoveries.len(), 1);
        assert_eq!(
            discoveries[0].session_key, "rollout-today",
            "should find session from any date dir, picking the most recent"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn get_cwd_via_lsof_invalid_pid_returns_none() {
        // PID 999999999 is almost certainly invalid
        let result = get_cwd_via_lsof(999999999);
        assert!(result.is_none());
    }

    #[test]
    fn discover_sessions_assigns_distinct_sessions_to_same_cwd_panes() {
        let tmp = temp_sessions_dir("discover-unique-same-cwd");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        let cwd = "/Users/vm/project";
        let old_path = write_session_meta(&day_dir, "rollout-old.jsonl", cwd);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let new_path = write_session_meta(&day_dir, "rollout-new.jsonl", cwd);

        let hints = vec![
            CodexPaneHint {
                pane_id: "%1".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: None,
                pane_generation: None,
                pane_birth_ts: None,
            },
            CodexPaneHint {
                pane_id: "%2".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: None,
                pane_generation: None,
                pane_birth_ts: None,
            },
        ];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(discoveries.len(), 2);

        let assigned_paths: HashSet<PathBuf> =
            discoveries.iter().map(|d| d.jsonl_path.clone()).collect();
        assert_eq!(
            assigned_paths.len(),
            2,
            "each pane should get a unique JSONL path"
        );
        assert!(assigned_paths.contains(&old_path));
        assert!(assigned_paths.contains(&new_path));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_reuses_existing_assignment_before_claiming_newer_file() {
        let tmp = temp_sessions_dir("discover-reuse-existing");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        let cwd = "/Users/vm/project";
        let old_path = write_session_meta(&day_dir, "rollout-old.jsonl", cwd);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let new_path = write_session_meta(&day_dir, "rollout-new.jsonl", cwd);

        let hints = vec![
            CodexPaneHint {
                pane_id: "%1".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: Some(old_path.clone()),
                pane_generation: None,
                pane_birth_ts: None,
            },
            CodexPaneHint {
                pane_id: "%2".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: None,
                pane_generation: None,
                pane_birth_ts: None,
            },
        ];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(discoveries.len(), 2);

        let pane1 = discoveries
            .iter()
            .find(|d| d.pane_id == "%1")
            .expect("pane1 discovery");
        let pane2 = discoveries
            .iter()
            .find(|d| d.pane_id == "%2")
            .expect("pane2 discovery");

        assert_eq!(pane1.jsonl_path, old_path);
        assert_eq!(pane2.jsonl_path, new_path);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_sessions_does_not_duplicate_one_session_across_same_cwd_panes() {
        let tmp = temp_sessions_dir("discover-no-dup");
        let sessions_dir = tmp.join("sessions");
        let day_dir = sessions_dir.join("2026").join("03").join("01");
        fs::create_dir_all(&day_dir).expect("test");

        let cwd = "/Users/vm/project";
        write_session_meta(&day_dir, "rollout-only.jsonl", cwd);

        let hints = vec![
            CodexPaneHint {
                pane_id: "%1".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: None,
                pane_generation: None,
                pane_birth_ts: None,
            },
            CodexPaneHint {
                pane_id: "%2".to_owned(),
                pane_pid: None,
                cwd: cwd.to_owned(),
                existing_jsonl_path: None,
                pane_generation: None,
                pane_birth_ts: None,
            },
        ];

        let discoveries = discover_sessions_in_dir(&hints, &sessions_dir);
        assert_eq!(
            discoveries.len(),
            1,
            "one transcript must not be bound to multiple same-CWD panes"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn canonicalize_path_tmp_fallback_substitutes_private_tmp() {
        // On macOS, /tmp is a symlink to /private/tmp.
        // If the path doesn't exist, fs::canonicalize fails → manual substitution kicks in.
        // /tmp/agtmux-nonexistent-test-path is virtually guaranteed not to exist.
        let input = "/tmp/agtmux-nonexistent-test-path-12345";
        let result = canonicalize_path(input);
        // The result must start with /private/tmp/ (manual substitution branch)
        // OR be the canonicalized form (if somehow the path exists on this machine — extremely unlikely).
        assert!(
            result == "/private/tmp/agtmux-nonexistent-test-path-12345" || result == input,
            "expected /private/tmp/... substitution or passthrough, got: {result}"
        );
        // On macOS the substitution should always fire for non-existent /tmp paths
        #[cfg(target_os = "macos")]
        assert_eq!(result, "/private/tmp/agtmux-nonexistent-test-path-12345");
    }
}
