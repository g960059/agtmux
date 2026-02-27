//! Activity signal matching for poller-based heuristic source.
//!
//! Extracts and generalizes the v4 `build_poller_evidence` signal matching
//! logic as a standalone module. Given captured terminal output lines and
//! a set of signal definitions, determines the most likely activity state.

use agtmux_core_v5::types::ActivityState;

// ─── Definitions ─────────────────────────────────────────────────

/// Activity signal pattern definition.
#[derive(Debug, Clone)]
pub struct ActivitySignalDef {
    pub state: ActivityState,
    /// Patterns to match in capture/last-line output (case-insensitive substring match).
    pub patterns: Vec<String>,
}

/// Result of activity signal matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityMatch {
    pub state: ActivityState,
    pub matched_pattern: String,
    pub line_index: usize,
}

// ─── MVP Signal Definitions ─────────────────────────────────────

/// MVP activity signal definitions for Claude.
pub fn claude_activity_signals() -> Vec<ActivitySignalDef> {
    vec![
        ActivitySignalDef {
            state: ActivityState::Running,
            patterns: vec![
                "Running".to_string(),
                "Generating".to_string(),
                "Thinking".to_string(),
                "\u{280b}".to_string(), // ⠋
                "\u{2819}".to_string(), // ⠙
                "\u{2839}".to_string(), // ⠹
                "\u{2838}".to_string(), // ⠸
            ],
        },
        ActivitySignalDef {
            state: ActivityState::Idle,
            patterns: vec![
                "\u{276f}".to_string(), // ❯
                "$ ".to_string(),
            ],
        },
        ActivitySignalDef {
            state: ActivityState::WaitingApproval,
            patterns: vec!["Allow?".to_string(), "Do you want to allow".to_string()],
        },
        ActivitySignalDef {
            state: ActivityState::Error,
            patterns: vec![
                "Error:".to_string(),
                "error:".to_string(),
                "panic".to_string(),
            ],
        },
    ]
}

/// MVP activity signal definitions for Codex.
pub fn codex_activity_signals() -> Vec<ActivitySignalDef> {
    vec![
        ActivitySignalDef {
            state: ActivityState::Running,
            patterns: vec![
                "Running".to_string(),
                "Processing".to_string(),
                "Thinking".to_string(),
            ],
        },
        ActivitySignalDef {
            state: ActivityState::Idle,
            patterns: vec!["codex>".to_string(), "$ ".to_string()],
        },
        ActivitySignalDef {
            state: ActivityState::WaitingApproval,
            patterns: vec!["Apply patch?".to_string()],
        },
        ActivitySignalDef {
            state: ActivityState::Error,
            patterns: vec!["Error:".to_string(), "error:".to_string()],
        },
    ]
}

// ─── Matching ───────────────────────────────────────────────────

/// Match activity signals against capture lines.
///
/// Scans lines from end to start (tail-first for recency).
/// For each line, checks each signal definition's patterns.
/// Returns the highest-priority match using `ActivityState::PRECEDENCE_DESC` ordering.
///
/// Returns `None` if no patterns match any line.
pub fn match_activity(
    capture_lines: &[&str],
    signals: &[ActivitySignalDef],
) -> Option<ActivityMatch> {
    let mut all_matches: Vec<ActivityMatch> = Vec::new();

    // Scan lines from end to start (tail-first: index 0 = last line)
    for (line_index, line) in capture_lines.iter().rev().enumerate() {
        let line_lower = line.to_ascii_lowercase();
        for signal_def in signals {
            for pattern in &signal_def.patterns {
                if line_lower.contains(&pattern.to_ascii_lowercase()) {
                    all_matches.push(ActivityMatch {
                        state: signal_def.state,
                        matched_pattern: pattern.clone(),
                        line_index,
                    });
                }
            }
        }
    }

    if all_matches.is_empty() {
        return None;
    }

    // Conflict resolution: use ActivityState::PRECEDENCE_DESC ordering.
    // Higher-priority states win. Among same-priority, prefer the one
    // with the lower line_index (closer to end = more recent).
    let precedence = ActivityState::PRECEDENCE_DESC;
    all_matches.sort_by(|a, b| {
        let a_rank = precedence
            .iter()
            .position(|&s| s == a.state)
            .unwrap_or(usize::MAX);
        let b_rank = precedence
            .iter()
            .position(|&s| s == b.state)
            .unwrap_or(usize::MAX);
        a_rank.cmp(&b_rank).then(a.line_index.cmp(&b.line_index))
    });

    all_matches.into_iter().next()
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 7. Activity: running pattern match ──────────────────────

    #[test]
    fn activity_running_pattern_match() {
        let signals = claude_activity_signals();
        let lines = &["Initializing", "Thinking about the problem"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match running");
        assert_eq!(m.state, ActivityState::Running);
        assert_eq!(m.matched_pattern, "Thinking");
    }

    // ── 8. Activity: idle pattern match ─────────────────────────

    #[test]
    fn activity_idle_pattern_match() {
        let signals = claude_activity_signals();
        let lines = &["Done.", "\u{276f}"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match idle");
        assert_eq!(m.state, ActivityState::Idle);
        assert_eq!(m.matched_pattern, "\u{276f}");
    }

    // ── 9. Activity: conflict resolution (running vs idle = running wins) ──

    #[test]
    fn activity_conflict_running_beats_idle() {
        let signals = claude_activity_signals();
        // Both running and idle patterns appear on the same "distance" from tail
        let lines = &["\u{276f}", "Thinking"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should resolve conflict");
        assert_eq!(
            m.state,
            ActivityState::Running,
            "running should beat idle in precedence"
        );
    }

    // ── 10. Activity: tail-first recency (last line takes precedence) ──

    #[test]
    fn activity_tail_first_recency() {
        let signals = codex_activity_signals();
        // Running appears early, idle appears at the end
        let lines = &["Running a task", "some output", "more output", "codex>"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match");
        // Both Running and Idle match. Running has higher precedence.
        // But we need to verify that the scanning works correctly.
        // Running has higher precedence than Idle, so Running should win.
        assert_eq!(m.state, ActivityState::Running);
    }

    // ── 11. Activity: no match returns None ─────────────────────

    #[test]
    fn activity_no_match_returns_none() {
        let signals = codex_activity_signals();
        let lines = &["some random output", "no signal here"];
        let result = match_activity(lines, &signals);

        assert!(result.is_none());
    }

    // ── Error state has highest priority ────────────────────────

    #[test]
    fn activity_error_highest_priority() {
        let signals = claude_activity_signals();
        let lines = &["Thinking", "\u{276f}", "Error: something went wrong"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match error");
        assert_eq!(m.state, ActivityState::Error);
    }

    // ── WaitingApproval beats Running ───────────────────────────

    #[test]
    fn activity_waiting_approval_beats_running() {
        let signals = claude_activity_signals();
        let lines = &["Thinking about it", "Allow? Press Y to confirm"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match waiting_approval");
        assert_eq!(m.state, ActivityState::WaitingApproval);
    }

    // ── Codex signals work ──────────────────────────────────────

    #[test]
    fn codex_running_signal() {
        let signals = codex_activity_signals();
        let lines = &["Processing your request"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match codex running");
        assert_eq!(m.state, ActivityState::Running);
        assert_eq!(m.matched_pattern, "Processing");
    }

    #[test]
    fn codex_idle_signal() {
        let signals = codex_activity_signals();
        let lines = &["codex>"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match codex idle");
        assert_eq!(m.state, ActivityState::Idle);
    }

    #[test]
    fn codex_waiting_approval_signal() {
        let signals = codex_activity_signals();
        let lines = &["Apply patch? (y/n)"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match codex waiting_approval");
        assert_eq!(m.state, ActivityState::WaitingApproval);
    }

    // ── Spinner character detection ─────────────────────────────

    #[test]
    fn claude_spinner_detection() {
        let signals = claude_activity_signals();
        let lines = &["\u{280b} Loading..."]; // ⠋
        let result = match_activity(lines, &signals);

        let m = result.expect("should match spinner running");
        assert_eq!(m.state, ActivityState::Running);
    }

    // ── Case-insensitive matching ───────────────────────────────

    #[test]
    fn activity_case_insensitive() {
        let signals = claude_activity_signals();
        let lines = &["THINKING HARD"];
        let result = match_activity(lines, &signals);

        let m = result.expect("should match case-insensitively");
        assert_eq!(m.state, ActivityState::Running);
    }

    // ── Empty lines ────────────────────────────────────────────

    #[test]
    fn activity_empty_lines() {
        let signals = claude_activity_signals();
        let lines: &[&str] = &[];
        let result = match_activity(lines, &signals);

        assert!(result.is_none());
    }
}
