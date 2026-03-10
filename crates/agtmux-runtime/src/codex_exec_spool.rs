//! Pane-bound synthetic Codex JSONL spool for `codex exec --json` output.
//!
//! This keeps CodexJsonl as the only deterministic Codex semantic path by
//! normalizing exec NDJSON into transcript-like JSONL lines and binding the
//! resulting spool file directly to the exact pane.
//!
//! The spool is intentionally pane-scoped, not invocation-scoped: consecutive
//! `codex exec` runs in the same tmux pane append to the same file and reuse
//! the same `session_key_override = codex:%pane`. That keeps reducer history
//! stable across repeated execs while still letting managed shell demotion tear
//! down the row when the pane returns to a plain shell.
//!
//! The current normalizer targets the `codex exec --json` / `codex --yolo`
//! command-execution flow used by live product proofs. Review-mode and aborted
//! exec events remain follow-up work; they do not affect the `sleep 30`
//! running-state parity fixed by this path.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecSpoolHint {
    pub jsonl_path: PathBuf,
    pub session_key_override: String,
}

#[derive(Debug)]
pub struct CodexExecSpoolTracker {
    spool_path: PathBuf,
    seen_raw_hashes: HashSet<u64>,
    has_semantic_lines: bool,
}

impl CodexExecSpoolTracker {
    pub fn new(spool_path: PathBuf) -> Self {
        Self {
            has_semantic_lines: spool_has_semantic_lines(&spool_path),
            spool_path,
            seen_raw_hashes: HashSet::new(),
        }
    }

    pub fn spool_path(&self) -> &Path {
        &self.spool_path
    }

    pub fn sync_capture(
        &mut self,
        cwd: &str,
        joined_capture_lines: &[String],
        observed_at: DateTime<Utc>,
    ) -> std::io::Result<()> {
        let mut normalized = Vec::new();

        for raw_line in joined_capture_lines {
            let Some(lines) = normalize_exec_ndjson_line(raw_line, observed_at) else {
                continue;
            };
            let hash = stable_line_hash(raw_line);
            if !self.seen_raw_hashes.insert(hash) {
                continue;
            }
            normalized.extend(lines);
        }

        if normalized.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.spool_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let need_header = !self.spool_path.exists() || fs::metadata(&self.spool_path)?.len() == 0;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.spool_path)?;

        if need_header {
            writeln!(
                file,
                "{}",
                serde_json::to_string(&session_meta_line(cwd)).expect("session meta serializes")
            )?;
        }

        for line in normalized {
            writeln!(file, "{line}")?;
        }

        self.has_semantic_lines = true;
        Ok(())
    }

    pub fn discovery_hint(&self, pane_id: &str) -> Option<CodexExecSpoolHint> {
        self.has_semantic_lines.then(|| CodexExecSpoolHint {
            jsonl_path: self.spool_path.clone(),
            session_key_override: format!("codex:{pane_id}"),
        })
    }
}

pub fn spool_path_for_pane(
    session_name: &str,
    window_id: &str,
    pane_id: &str,
    pane_pid: Option<u32>,
) -> PathBuf {
    let root = spool_root();
    let socket_component = spool_socket_component();
    let pane_component = sanitize_path_component(pane_id);
    let session_component = sanitize_path_component(session_name);
    let window_component = sanitize_path_component(window_id);
    let pid_component = pane_pid
        .map(|pid| format!("pid{pid}"))
        .unwrap_or_else(|| "pid-none".to_owned());

    // Keep the spool path stable across daemon restarts. PaneGenerationTracker
    // birth_ts is daemon-observed wall clock, not tmux-native creation time, so
    // encoding generation/birth into the filename would orphan the pre-restart
    // spool and break historical rehydration.
    root.join(socket_component).join(format!(
        "{session_component}-{window_component}-{pane_component}-{pid_component}.jsonl"
    ))
}

fn spool_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join(".agtmux")
                .join("codex-exec-spool");
        }
    }
    std::env::temp_dir().join("agtmux-codex-exec-spool")
}

fn spool_socket_component() -> String {
    if let Ok(path) = std::env::var("AGTMUX_TMUX_SOCKET_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return sanitize_path_component(trimmed);
        }
    }
    if let Ok(name) = std::env::var("AGTMUX_TMUX_SOCKET_NAME") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return sanitize_path_component(trimmed);
        }
    }
    "default".to_owned()
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

fn session_meta_line(cwd: &str) -> serde_json::Value {
    json!({
        "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "type": "session_meta",
        "payload": {
            "type": "session_meta",
            "cwd": cwd,
            "sessionId": "agtmux-codex-exec-spool"
        }
    })
}

fn normalize_exec_ndjson_line(raw_line: &str, observed_at: DateTime<Utc>) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(raw_line.trim()).ok()?;
    let top_type = value.get("type")?.as_str()?;
    let ts = observed_at.to_rfc3339_opts(SecondsFormat::Millis, true);

    let normalized = match top_type {
        "thread.started" | "turn.started" => {
            vec![json!({
                "timestamp": ts,
                "type": "event_msg",
                "payload": {
                    "type": "task_started"
                }
            })]
        }
        "item.started"
            if value["item"]["type"].as_str() == Some("command_execution")
                && value["item"]["status"].as_str() == Some("in_progress") =>
        {
            let call_id = value["item"]["id"].as_str()?;
            vec![json!({
                "timestamp": ts,
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "call_id": call_id,
                    "name": "shell_command"
                }
            })]
        }
        "item.completed" if value["item"]["type"].as_str() == Some("command_execution") => {
            let call_id = value["item"]["id"].as_str()?;
            vec![json!({
                "timestamp": ts,
                "type": "response_item",
                "payload": {
                    "type": "function_call_output",
                    "call_id": call_id,
                    "name": "shell_command"
                }
            })]
        }
        "turn.completed" => {
            vec![json!({
                "timestamp": ts,
                "type": "event_msg",
                "payload": {
                    "type": "task_complete"
                }
            })]
        }
        _ => return None,
    };

    normalized
        .into_iter()
        .map(|line| serde_json::to_string(&line).ok())
        .collect()
}

fn spool_has_semantic_lines(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    text.lines().skip(1).any(|line| !line.trim().is_empty())
}

fn stable_line_hash(line: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 10, 17, 0, 0)
            .single()
            .expect("valid datetime")
    }

    fn temp_spool_path(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-codex-exec-spool-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("test");
        dir.join("pane.jsonl")
    }

    #[test]
    fn normalize_exec_ndjson_maps_running_tool_and_completion_events() {
        let ts = now();

        let task_started =
            normalize_exec_ndjson_line(r#"{"type":"thread.started","thread_id":"t"}"#, ts)
                .expect("task started normalized");
        assert_eq!(task_started.len(), 1);
        assert!(task_started[0].contains(r#""type":"event_msg""#));
        assert!(task_started[0].contains(r#""task_started""#));

        let tool_started = normalize_exec_ndjson_line(
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","status":"in_progress","command":"sleep 30"}}"#,
            ts,
        )
        .expect("tool started normalized");
        assert_eq!(tool_started.len(), 1);
        assert!(tool_started[0].contains(r#""function_call""#));
        assert!(tool_started[0].contains(r#""call_id":"item_1""#));

        let tool_completed = normalize_exec_ndjson_line(
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","status":"completed","exit_code":0}}"#,
            ts,
        )
        .expect("tool completed normalized");
        assert_eq!(tool_completed.len(), 1);
        assert!(tool_completed[0].contains(r#""function_call_output""#));

        let task_completed = normalize_exec_ndjson_line(r#"{"type":"turn.completed"}"#, ts)
            .expect("task complete normalized");
        assert_eq!(task_completed.len(), 1);
        assert!(task_completed[0].contains(r#""task_complete""#));
    }

    #[test]
    fn tracker_writes_session_meta_once_and_dedups_raw_lines() {
        let path = temp_spool_path("dedup");
        let mut tracker = CodexExecSpoolTracker::new(path.clone());
        let capture_lines = vec![
            r#"{"type":"thread.started","thread_id":"t"}"#.to_owned(),
            r#"{"type":"thread.started","thread_id":"t"}"#.to_owned(),
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","status":"in_progress"}}"#.to_owned(),
        ];

        tracker
            .sync_capture("/Users/vm/project", &capture_lines, now())
            .expect("sync capture");
        tracker
            .sync_capture("/Users/vm/project", &capture_lines, now())
            .expect("dedup sync capture");

        let text = fs::read_to_string(&path).expect("spool file");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "session_meta + 2 semantic lines");
        assert!(lines[0].contains(r#""session_meta""#));
        assert!(lines[1].contains(r#""task_started""#));
        assert!(lines[2].contains(r#""function_call""#));

        let hint = tracker.discovery_hint("%1").expect("discovery hint");
        assert_eq!(hint.jsonl_path, path);
        assert_eq!(hint.session_key_override, "codex:%1");
    }

    #[test]
    fn spool_path_is_restart_stable_even_if_generation_changes() {
        let path_a = spool_path_for_pane("vm agtmux", "@0", "%1", Some(1234));
        let path_b = spool_path_for_pane("vm agtmux", "@0", "%1", Some(1234));
        assert_eq!(path_a, path_b);
    }
}
