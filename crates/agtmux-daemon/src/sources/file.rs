use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use agtmux_core::source::SourceEvent;
use agtmux_core::types::{
    ActivityState, Evidence, EvidenceKind, Provider, SourceType,
};
use chrono::Utc;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Configuration for a single directory watch.
#[derive(Debug, Clone)]
pub struct FileWatchConfig {
    /// Directory to watch (e.g., `~/.claude/`).
    pub path: PathBuf,
    /// Which provider this maps to.
    pub provider: Provider,
    /// Glob patterns for relevant files (e.g., `["*.jsonl"]`).
    pub patterns: Vec<String>,
    /// Pane ID to associate events with.
    pub pane_id: String,
}

impl FileWatchConfig {
    pub fn new(
        path: PathBuf,
        provider: Provider,
        patterns: Vec<String>,
        pane_id: String,
    ) -> Self {
        Self {
            path,
            provider,
            patterns,
            pane_id,
        }
    }
}

/// Monitors session files (e.g., `~/.claude/projects/*/` for Claude Code)
/// and emits `SourceEvent`s when file changes indicate state transitions.
pub struct FileSource {
    tx: mpsc::Sender<SourceEvent>,
    watch_dirs: Vec<FileWatchConfig>,
}

impl FileSource {
    pub fn new(tx: mpsc::Sender<SourceEvent>, watch_dirs: Vec<FileWatchConfig>) -> Self {
        Self { tx, watch_dirs }
    }

    /// Start watching configured directories. Blocks until cancelled.
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Channel to bridge synchronous notify callbacks into async land.
        let (notify_tx, mut notify_rx) = mpsc::channel::<notify::Result<Event>>(256);

        // Create a recommended watcher that sends events into our channel.
        let mut watcher: RecommendedWatcher = {
            let tx = notify_tx.clone();
            notify::recommended_watcher(move |res| {
                // Best-effort send; if the receiver is dropped we just stop.
                let _ = tx.blocking_send(res);
            })?
        };

        // Register each configured directory.
        for config in &self.watch_dirs {
            if !config.path.exists() {
                tracing::warn!(
                    path = %config.path.display(),
                    "file source: watch directory does not exist, skipping"
                );
                continue;
            }

            if let Err(e) = watcher.watch(&config.path, RecursiveMode::Recursive) {
                tracing::warn!(
                    path = %config.path.display(),
                    "file source: failed to watch directory: {e}"
                );
            } else {
                tracing::info!(
                    path = %config.path.display(),
                    provider = ?config.provider,
                    "file source: watching directory"
                );
            }
        }

        // Process events from the watcher.
        while let Some(event_result) = notify_rx.recv().await {
            match event_result {
                Ok(event) => {
                    self.handle_notify_event(&event).await;
                }
                Err(e) => {
                    tracing::warn!("file source: watcher error: {e}");
                }
            }
        }

        Ok(())
    }

    /// Process a single notify event: check if any watched config matches,
    /// then parse and emit a `SourceEvent`.
    async fn handle_notify_event(&self, event: &Event) {
        // Only process create and modify events.
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {}
            _ => return,
        }

        for path in &event.paths {
            // Find the matching watch config for this path.
            if let Some(config) = self.find_config_for_path(path) {
                // Check if the file matches one of the configured glob patterns.
                if !matches_any_pattern(path, &config.patterns) {
                    continue;
                }

                // Read the file and derive activity from its content.
                match read_last_line(path) {
                    Ok(Some(last_line)) => {
                        let activity = parse_session_line(&last_line, config.provider);
                        let evidence = build_file_evidence(
                            config.provider,
                            activity,
                            path,
                            &last_line,
                        );

                        let event = SourceEvent::Evidence {
                            pane_id: config.pane_id.clone(),
                            evidence: vec![evidence],
                            meta: None,
                        };

                        if let Err(e) = self.tx.send(event).await {
                            tracing::warn!(
                                path = %path.display(),
                                "file source: failed to send event: {e}"
                            );
                        }
                    }
                    Ok(None) => {
                        tracing::debug!(path = %path.display(), "file source: file is empty");
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            "file source: failed to read file: {e}"
                        );
                    }
                }
            }
        }
    }

    /// Find the `FileWatchConfig` whose `path` is a prefix of the given file path.
    fn find_config_for_path(&self, file_path: &std::path::Path) -> Option<&FileWatchConfig> {
        self.watch_dirs
            .iter()
            .find(|config| file_path.starts_with(&config.path))
    }
}

/// Check whether a file path matches any of the given glob patterns.
///
/// The patterns are matched against the file name component only
/// (e.g., `"*.jsonl"` matches `session-abc.jsonl`).
fn matches_any_pattern(path: &std::path::Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        // No patterns means accept everything.
        return true;
    }

    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    for pattern in patterns {
        match glob::Pattern::new(pattern) {
            Ok(pat) if pat.matches(file_name) => return true,
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(pattern = %pattern, "invalid glob pattern: {e}");
            }
        }
    }
    false
}

/// Read the last non-empty line of a file.
fn read_last_line(
    path: &std::path::Path,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;

    if metadata.len() == 0 {
        return Ok(None);
    }

    let reader = std::io::BufReader::new(file);
    let mut last_line: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            last_line = Some(trimmed);
        }
    }

    Ok(last_line)
}

/// Parse a session file line to determine the current activity state.
///
/// For Claude Code session files (`.jsonl` format), each line is a JSON
/// object. We inspect the `"type"` field to infer agent activity:
///
/// - `"tool_use"` -> Running (agent is executing a tool)
/// - `"ask_permission"` -> WaitingApproval (agent needs user confirmation)
/// - Other recognized types may be added later
/// - Default -> Unknown
pub fn parse_session_line(line: &str, provider: Provider) -> ActivityState {
    match provider {
        Provider::Claude => parse_claude_session_line(line),
        // Other providers can add their own parsers here.
        _ => ActivityState::Unknown,
    }
}

/// Parse a Claude Code session `.jsonl` line.
fn parse_claude_session_line(line: &str) -> ActivityState {
    // Try to parse as JSON and inspect the "type" field.
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            // Not valid JSON — try simple substring matching as fallback.
            return parse_claude_line_heuristic(line);
        }
    };

    match value.get("type").and_then(|v| v.as_str()) {
        Some("tool_use") => ActivityState::Running,
        Some("ask_permission") => ActivityState::WaitingApproval,
        Some("tool_result") => ActivityState::Running,
        Some("error") => ActivityState::Error,
        Some("result") => ActivityState::Idle,
        Some("text") => ActivityState::Running,
        _ => ActivityState::Unknown,
    }
}

/// Heuristic fallback: search for known substrings in the line.
fn parse_claude_line_heuristic(line: &str) -> ActivityState {
    if line.contains("\"type\":\"tool_use\"") || line.contains("\"type\": \"tool_use\"") {
        ActivityState::Running
    } else if line.contains("\"type\":\"ask_permission\"")
        || line.contains("\"type\": \"ask_permission\"")
    {
        ActivityState::WaitingApproval
    } else if line.contains("\"type\":\"error\"") || line.contains("\"type\": \"error\"") {
        ActivityState::Error
    } else {
        ActivityState::Unknown
    }
}

/// Build an `Evidence` from a file change event.
pub fn build_file_evidence(
    provider: Provider,
    signal: ActivityState,
    path: &std::path::Path,
    _last_line: &str,
) -> Evidence {
    let path_str = path.display().to_string();
    let now = Utc::now();

    // Weight and confidence depend on the signal.
    let (weight, confidence) = match signal {
        ActivityState::Running => (0.80, 0.75),
        ActivityState::WaitingApproval => (0.85, 0.80),
        ActivityState::Idle => (0.70, 0.70),
        ActivityState::Error => (0.85, 0.80),
        ActivityState::WaitingInput => (0.80, 0.75),
        ActivityState::Unknown => (0.50, 0.50),
        // Handle future variants added to the non-exhaustive enum.
        _ => (0.50, 0.50),
    };

    Evidence {
        provider,
        kind: EvidenceKind::FileChange(path_str.clone()),
        signal,
        weight,
        confidence,
        timestamp: now,
        ttl: Duration::from_secs(60),
        source: SourceType::File,
        reason_code: format!("file:{}", path_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: FileWatchConfig creation
    // -----------------------------------------------------------------------

    #[test]
    fn file_watch_config_creation() {
        let config = FileWatchConfig::new(
            PathBuf::from("/home/user/.claude"),
            Provider::Claude,
            vec!["*.jsonl".to_string()],
            "%1".to_string(),
        );

        assert_eq!(config.path, PathBuf::from("/home/user/.claude"));
        assert!(matches!(config.provider, Provider::Claude));
        assert_eq!(config.patterns, vec!["*.jsonl".to_string()]);
        assert_eq!(config.pane_id, "%1");
    }

    #[test]
    fn file_watch_config_with_multiple_patterns() {
        let config = FileWatchConfig::new(
            PathBuf::from("/tmp/sessions"),
            Provider::Codex,
            vec!["*.jsonl".to_string(), "*.log".to_string()],
            "%2".to_string(),
        );

        assert_eq!(config.patterns.len(), 2);
        assert!(matches!(config.provider, Provider::Codex));
    }

    // -----------------------------------------------------------------------
    // Test 2: File event to SourceEvent conversion
    // -----------------------------------------------------------------------

    #[test]
    fn build_evidence_produces_source_event_with_correct_fields() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::Running,
            std::path::Path::new("/home/user/.claude/projects/foo/session.jsonl"),
            r#"{"type":"tool_use","tool":"Bash"}"#,
        );

        let event = SourceEvent::Evidence {
            pane_id: "%1".to_string(),
            evidence: vec![evidence],
            meta: None,
        };

        match event {
            SourceEvent::Evidence { pane_id, evidence, .. } => {
                assert_eq!(pane_id, "%1");
                assert_eq!(evidence.len(), 1);
                let ev = &evidence[0];
                assert!(matches!(ev.provider, Provider::Claude));
                assert!(matches!(ev.signal, ActivityState::Running));
                assert_eq!(ev.weight, 0.80);
                assert_eq!(ev.confidence, 0.75);
            }
            other => panic!("expected SourceEvent::Evidence, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: Claude session file parsing (last line JSON patterns)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_claude_tool_use_returns_running() {
        let line = r#"{"type":"tool_use","tool":"Bash","input":{"command":"ls"}}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Running
        );
    }

    #[test]
    fn parse_claude_ask_permission_returns_waiting_approval() {
        let line = r#"{"type":"ask_permission","tool":"Write","path":"/tmp/foo"}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::WaitingApproval
        );
    }

    #[test]
    fn parse_claude_result_returns_idle() {
        let line = r#"{"type":"result","result":"done"}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Idle
        );
    }

    #[test]
    fn parse_claude_error_returns_error() {
        let line = r#"{"type":"error","message":"rate limited"}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Error
        );
    }

    #[test]
    fn parse_claude_tool_result_returns_running() {
        let line = r#"{"type":"tool_result","output":"success"}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Running
        );
    }

    #[test]
    fn parse_claude_text_returns_running() {
        let line = r#"{"type":"text","content":"thinking..."}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Running
        );
    }

    #[test]
    fn parse_claude_unknown_type_returns_unknown() {
        let line = r#"{"type":"something_else","data":42}"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Unknown
        );
    }

    #[test]
    fn parse_non_json_uses_heuristic() {
        // Contains tool_use substring but is not valid JSON.
        let line = r#"broken json "type":"tool_use" stuff"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::Running
        );
    }

    #[test]
    fn parse_non_json_heuristic_ask_permission() {
        let line = r#"not json "type":"ask_permission" blah"#;
        assert_eq!(
            parse_session_line(line, Provider::Claude),
            ActivityState::WaitingApproval
        );
    }

    #[test]
    fn parse_non_claude_provider_returns_unknown() {
        let line = r#"{"type":"tool_use","tool":"Bash"}"#;
        assert_eq!(
            parse_session_line(line, Provider::Codex),
            ActivityState::Unknown
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Graceful handling of missing directories
    // -----------------------------------------------------------------------

    #[test]
    fn find_config_returns_none_for_unmatched_path() {
        let (tx, _rx) = mpsc::channel(16);
        let source = FileSource::new(
            tx,
            vec![FileWatchConfig::new(
                PathBuf::from("/home/user/.claude"),
                Provider::Claude,
                vec!["*.jsonl".to_string()],
                "%1".to_string(),
            )],
        );

        // A path that does NOT start with the configured watch directory.
        let result = source.find_config_for_path(std::path::Path::new("/other/path/file.jsonl"));
        assert!(result.is_none());
    }

    #[test]
    fn find_config_returns_some_for_matched_path() {
        let (tx, _rx) = mpsc::channel(16);
        let source = FileSource::new(
            tx,
            vec![FileWatchConfig::new(
                PathBuf::from("/home/user/.claude"),
                Provider::Claude,
                vec!["*.jsonl".to_string()],
                "%1".to_string(),
            )],
        );

        let result = source
            .find_config_for_path(std::path::Path::new("/home/user/.claude/projects/session.jsonl"));
        assert!(result.is_some());
        assert!(matches!(result.unwrap().provider, Provider::Claude));
    }

    #[test]
    fn matches_any_pattern_with_matching_glob() {
        let path = std::path::Path::new("/some/dir/session.jsonl");
        let patterns = vec!["*.jsonl".to_string()];
        assert!(matches_any_pattern(path, &patterns));
    }

    #[test]
    fn matches_any_pattern_with_no_match() {
        let path = std::path::Path::new("/some/dir/session.log");
        let patterns = vec!["*.jsonl".to_string()];
        assert!(!matches_any_pattern(path, &patterns));
    }

    #[test]
    fn matches_any_pattern_empty_patterns_matches_all() {
        let path = std::path::Path::new("/some/dir/anything.txt");
        let patterns: Vec<String> = vec![];
        assert!(matches_any_pattern(path, &patterns));
    }

    // -----------------------------------------------------------------------
    // Test 5: SourceType::File is used in emitted evidence
    // -----------------------------------------------------------------------

    #[test]
    fn evidence_uses_source_type_file() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::Running,
            std::path::Path::new("/tmp/session.jsonl"),
            r#"{"type":"tool_use"}"#,
        );

        assert!(
            matches!(evidence.source, SourceType::File),
            "expected SourceType::File, got {:?}",
            evidence.source,
        );
    }

    #[test]
    fn evidence_kind_is_file_change() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::WaitingApproval,
            std::path::Path::new("/tmp/session.jsonl"),
            r#"{"type":"ask_permission"}"#,
        );

        assert!(
            matches!(evidence.kind, EvidenceKind::FileChange(ref path) if path.contains("session.jsonl")),
            "expected EvidenceKind::FileChange containing file path, got {:?}",
            evidence.kind,
        );
    }

    #[test]
    fn evidence_ttl_is_60_seconds() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::Idle,
            std::path::Path::new("/tmp/session.jsonl"),
            r#"{"type":"result"}"#,
        );

        assert_eq!(evidence.ttl, Duration::from_secs(60));
    }

    #[test]
    fn evidence_reason_code_contains_file_path() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::Running,
            std::path::Path::new("/home/user/.claude/session.jsonl"),
            r#"{"type":"tool_use"}"#,
        );

        assert!(
            evidence.reason_code.starts_with("file:"),
            "reason_code should start with 'file:', got: {}",
            evidence.reason_code,
        );
        assert!(
            evidence.reason_code.contains("session.jsonl"),
            "reason_code should contain file name, got: {}",
            evidence.reason_code,
        );
    }

    #[test]
    fn evidence_weight_and_confidence_for_unknown() {
        let evidence = build_file_evidence(
            Provider::Claude,
            ActivityState::Unknown,
            std::path::Path::new("/tmp/x.jsonl"),
            "garbage",
        );

        assert_eq!(evidence.weight, 0.50);
        assert_eq!(evidence.confidence, 0.50);
    }

    // -----------------------------------------------------------------------
    // read_last_line tests
    // -----------------------------------------------------------------------

    #[test]
    fn read_last_line_from_temp_file() {
        let dir = std::env::temp_dir().join("agtmux-test-file-source");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("test_session.jsonl");

        std::fs::write(
            &file_path,
            r#"{"type":"text","content":"hello"}
{"type":"tool_use","tool":"Bash"}
"#,
        )
        .unwrap();

        let result = read_last_line(&file_path).unwrap();
        assert_eq!(
            result,
            Some(r#"{"type":"tool_use","tool":"Bash"}"#.to_string())
        );

        // Cleanup.
        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn read_last_line_empty_file() {
        let dir = std::env::temp_dir().join("agtmux-test-file-source-empty");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("empty.jsonl");

        std::fs::write(&file_path, "").unwrap();

        let result = read_last_line(&file_path).unwrap();
        assert!(result.is_none());

        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn read_last_line_nonexistent_file() {
        let result = read_last_line(std::path::Path::new("/nonexistent/path/file.jsonl"));
        assert!(result.is_err());
    }
}
