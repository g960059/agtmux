//! Agent detection (pattern matching) for poller-based heuristic source.
//!
//! Extracts and generalizes the v4 `detect_with_provider_def` logic as a
//! standalone module usable by the v5 poller source server.

use agtmux_core_v5::signature::{
    WEIGHT_CMD_MATCH, WEIGHT_POLLER_MATCH, WEIGHT_PROCESS_HINT, WEIGHT_TITLE_MATCH,
};
use agtmux_core_v5::types::Provider;

// ─── Definitions ─────────────────────────────────────────────────

/// Provider detection definition (MVP: hardcoded for Claude and Codex).
#[derive(Debug, Clone)]
pub struct ProviderDetectDef {
    pub provider: Provider,
    /// Process name hint (e.g. "claude", "codex").
    pub process_hint: &'static str,
    /// Command tokens to match in current_cmd (case-insensitive).
    pub cmd_tokens: &'static [&'static str],
    /// Title tokens to match in pane_title (case-insensitive).
    pub title_tokens: &'static [&'static str],
    /// Capture tokens to match in terminal output (case-insensitive).
    pub capture_tokens: &'static [&'static str],
    /// Whether the cmd is typically run via a wrapper (node/bun/deno).
    pub wrapper_cmd: bool,
}

/// Detection result for a single provider check.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectResult {
    pub provider: Provider,
    pub provider_hint: bool,
    pub cmd_match: bool,
    pub title_match: bool,
    pub capture_match: bool,
    pub is_wrapper_cmd: bool,
    /// Maximum confidence score from matched signals.
    pub confidence: f64,
}

/// Pane metadata input for detection.
#[derive(Debug, Clone, Default)]
pub struct PaneMeta {
    pub pane_title: String,
    pub current_cmd: String,
    /// Optional process name hint from process inspection.
    pub process_hint: Option<String>,
    /// Captured terminal output lines for capture-based detection.
    pub capture_lines: Vec<String>,
}

// ─── MVP Provider Definitions ────────────────────────────────────

/// MVP provider definitions for Claude and Codex.
/// Known shell command names for stale title suppression.
/// Case-insensitive basename matching is used.
const KNOWN_SHELLS: &[&str] = &[
    "zsh", "bash", "fish", "sh", "dash", "nu", "pwsh", "tcsh", "csh", "ksh", "ash",
];

pub fn mvp_provider_defs() -> Vec<ProviderDetectDef> {
    vec![
        ProviderDetectDef {
            provider: Provider::Claude,
            process_hint: "claude",
            cmd_tokens: &["claude"],
            title_tokens: &["claude", "claude code"],
            capture_tokens: &["claude code", "\u{256D} Claude Code"],
            wrapper_cmd: false,
        },
        ProviderDetectDef {
            provider: Provider::Codex,
            process_hint: "codex",
            cmd_tokens: &["codex"],
            title_tokens: &["codex", "openai codex"],
            capture_tokens: &["codex>"],
            wrapper_cmd: true,
        },
    ]
}

// ─── Detection ──────────────────────────────────────────────────

/// Detect agent presence in a pane for a given provider definition.
///
/// Returns `Some(DetectResult)` if at least one signal matches,
/// or `None` if no signals match.
pub fn detect(meta: &PaneMeta, def: &ProviderDetectDef) -> Option<DetectResult> {
    let mut confidence: f64 = 0.0;
    let mut provider_hint = false;
    let mut cmd_match = false;
    let mut title_match = false;
    let mut capture_match = false;

    // 1. Check process_hint
    if let Some(ref hint) = meta.process_hint
        && hint.to_ascii_lowercase().contains(def.process_hint)
    {
        provider_hint = true;
        confidence = f64_max(confidence, WEIGHT_PROCESS_HINT);
    }

    // 2. Check cmd_tokens
    let current_cmd_lower = meta.current_cmd.to_ascii_lowercase();
    for token in def.cmd_tokens {
        if current_cmd_lower.contains(&token.to_ascii_lowercase()) {
            cmd_match = true;
            confidence = f64_max(confidence, WEIGHT_CMD_MATCH);
            break;
        }
    }

    // 3. Check title_tokens
    let pane_title_lower = meta.pane_title.to_ascii_lowercase();
    for token in def.title_tokens {
        if pane_title_lower.contains(&token.to_ascii_lowercase()) {
            title_match = true;
            confidence = f64_max(confidence, WEIGHT_TITLE_MATCH);
            break;
        }
    }

    // 4. Check capture_tokens (4th signal)
    if !def.capture_tokens.is_empty() {
        'capture: for line in &meta.capture_lines {
            let line_lower = line.to_ascii_lowercase();
            for token in def.capture_tokens {
                if line_lower.contains(&token.to_ascii_lowercase()) {
                    capture_match = true;
                    confidence = f64_max(confidence, WEIGHT_POLLER_MATCH);
                    break 'capture;
                }
            }
        }
    }

    // 5. If no matches, return None
    if !provider_hint && !cmd_match && !title_match && !capture_match {
        return None;
    }

    // 6. Stale title suppression: title_match only + shell cmd + no capture → None
    if title_match && !provider_hint && !cmd_match && !capture_match {
        let cmd_basename = cmd_basename(&meta.current_cmd);
        if is_known_shell(&cmd_basename) {
            return None;
        }
    }

    // 7. Return DetectResult
    Some(DetectResult {
        provider: def.provider,
        provider_hint,
        cmd_match,
        title_match,
        capture_match,
        is_wrapper_cmd: def.wrapper_cmd,
        confidence,
    })
}

/// Extract the basename from a command path, stripping login-shell prefix.
/// e.g., "/usr/local/bin/fish" → "fish", "Zsh" → "zsh", "-zsh" → "zsh"
fn cmd_basename(cmd: &str) -> String {
    let trimmed = cmd.trim();
    let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
    // Also handle spaces (take first word if there are arguments)
    let first_word = basename.split_whitespace().next().unwrap_or(basename);
    // Strip login-shell prefix '-' (e.g. "-zsh" → "zsh")
    let stripped = first_word.strip_prefix('-').unwrap_or(first_word);
    stripped.to_ascii_lowercase()
}

/// Check if a command basename is a known shell.
fn is_known_shell(basename: &str) -> bool {
    KNOWN_SHELLS.contains(&basename)
}

/// Detect across all MVP providers, return the best match (highest confidence).
///
/// When two providers score equal confidence, tie-breaks on signal strength:
/// `(provider_hint, cmd_match, title_match)` — richer evidence wins.
///
/// Returns `None` if no provider matches.
pub fn detect_best(meta: &PaneMeta) -> Option<DetectResult> {
    let defs = mvp_provider_defs();
    let mut best: Option<DetectResult> = None;

    for def in &defs {
        if let Some(result) = detect(meta, def) {
            let dominated = best.as_ref().is_some_and(|current| {
                if current.confidence > result.confidence {
                    true
                } else if current.confidence == result.confidence {
                    // Tie-break: prefer richer signal evidence.
                    let current_strength = (
                        current.provider_hint,
                        current.cmd_match,
                        current.capture_match,
                        current.title_match,
                    );
                    let candidate_strength = (
                        result.provider_hint,
                        result.cmd_match,
                        result.capture_match,
                        result.title_match,
                    );
                    current_strength >= candidate_strength
                } else {
                    false
                }
            });
            if !dominated {
                best = Some(result);
            }
        }
    }

    best
}

/// f64 does not implement Ord, so we provide a simple max helper.
fn f64_max(a: f64, b: f64) -> f64 {
    if a >= b { a } else { b }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. Detection: Claude process hint match ──────────────────

    #[test]
    fn detect_claude_process_hint() {
        let meta = PaneMeta {
            pane_title: String::new(),
            current_cmd: String::new(),
            process_hint: Some("claude".to_string()),
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        let r = result.expect("should detect Claude");
        assert_eq!(r.provider, Provider::Claude);
        assert!(r.provider_hint);
        assert!((r.confidence - WEIGHT_PROCESS_HINT).abs() < f64::EPSILON);
    }

    // ── 2. Detection: Codex cmd token match ─────────────────────

    #[test]
    fn detect_codex_cmd_token() {
        let meta = PaneMeta {
            pane_title: String::new(),
            current_cmd: "codex --model o3".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let codex_def = defs.iter().find(|d| d.provider == Provider::Codex);
        let result = detect(&meta, codex_def.expect("codex def should exist"));

        let r = result.expect("should detect Codex");
        assert_eq!(r.provider, Provider::Codex);
        assert!(r.cmd_match);
        assert!(!r.provider_hint);
        assert!((r.confidence - WEIGHT_CMD_MATCH).abs() < f64::EPSILON);
    }

    // ── 3. Detection: title-only match (lower confidence) ───────

    #[test]
    fn detect_title_only_lower_confidence() {
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        let r = result.expect("should detect via title");
        assert!(r.title_match);
        assert!(!r.provider_hint);
        assert!(!r.cmd_match);
        assert!((r.confidence - WEIGHT_TITLE_MATCH).abs() < f64::EPSILON);
    }

    // ── 4. Detection: no match returns None ─────────────────────

    #[test]
    fn detect_no_match_returns_none() {
        let meta = PaneMeta {
            pane_title: "vim".to_string(),
            current_cmd: "bash".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        assert!(result.is_none());
    }

    // ── 5. Detection: best match picks highest confidence ───────

    #[test]
    fn detect_best_picks_highest_confidence() {
        // Meta that matches Claude via process_hint (1.0) and Codex via title (0.66)
        let meta = PaneMeta {
            pane_title: "codex terminal".to_string(),
            current_cmd: String::new(),
            process_hint: Some("claude".to_string()),
            ..Default::default()
        };
        let result = detect_best(&meta);

        let r = result.expect("should detect best");
        assert_eq!(r.provider, Provider::Claude);
        assert!((r.confidence - WEIGHT_PROCESS_HINT).abs() < f64::EPSILON);
    }

    // ── 6. Detection: wrapper_cmd flag propagated ───────────────

    #[test]
    fn detect_wrapper_cmd_propagated() {
        let meta = PaneMeta {
            pane_title: "codex terminal".to_string(),
            current_cmd: String::new(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let codex_def = defs.iter().find(|d| d.provider == Provider::Codex);
        let result = detect(&meta, codex_def.expect("codex def should exist"));

        let r = result.expect("should detect Codex via title");
        assert!(r.is_wrapper_cmd, "Codex should have wrapper_cmd=true");

        // Claude should have wrapper_cmd=false
        let meta2 = PaneMeta {
            pane_title: "claude code".to_string(),
            current_cmd: String::new(),
            process_hint: None,
            ..Default::default()
        };
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result2 = detect(&meta2, claude_def.expect("claude def should exist"));

        let r2 = result2.expect("should detect Claude via title");
        assert!(!r2.is_wrapper_cmd, "Claude should have wrapper_cmd=false");
    }

    // ── Case-insensitive matching ───────────────────────────────

    #[test]
    fn detect_case_insensitive_cmd() {
        let meta = PaneMeta {
            pane_title: String::new(),
            current_cmd: "CLAUDE".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        assert!(result.is_some(), "case-insensitive cmd match should work");
    }

    #[test]
    fn detect_case_insensitive_title() {
        let meta = PaneMeta {
            pane_title: "OPENAI CODEX".to_string(),
            current_cmd: String::new(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let codex_def = defs.iter().find(|d| d.provider == Provider::Codex);
        let result = detect(&meta, codex_def.expect("codex def should exist"));

        assert!(result.is_some(), "case-insensitive title match should work");
    }

    // ── detect_best returns None when nothing matches ───────────

    #[test]
    fn detect_best_no_match() {
        let meta = PaneMeta {
            pane_title: "vim".to_string(),
            current_cmd: "bash".to_string(),
            process_hint: None,
            ..Default::default()
        };
        assert!(detect_best(&meta).is_none());
    }

    // ── Multiple signals: max confidence ────────────────────────

    #[test]
    fn detect_multiple_signals_max_confidence() {
        let meta = PaneMeta {
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            process_hint: Some("claude".to_string()),
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        let r = result.expect("should detect Claude");
        assert!(r.provider_hint);
        assert!(r.cmd_match);
        assert!(r.title_match);
        // confidence should be the max (process_hint = 1.0)
        assert!((r.confidence - WEIGHT_PROCESS_HINT).abs() < f64::EPSILON);
    }

    // ── Tie-breaking: richer signal evidence wins ────────────

    #[test]
    fn detect_best_tie_prefers_richer_signals() {
        // Both Claude and Codex match title-only with WEIGHT_TITLE_MATCH,
        // but Codex also has cmd_match (richer signal) → Codex should win.
        let meta = PaneMeta {
            pane_title: "claude codex".to_string(),
            current_cmd: "codex".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let result = detect_best(&meta);

        let r = result.expect("should detect something");
        assert_eq!(
            r.provider,
            Provider::Codex,
            "Codex has richer signals (cmd_match + title_match) and should win tie"
        );
        assert!(r.cmd_match);
        assert!(r.title_match);
    }

    // ── Capture-based detection tests ─────────────────────────────

    #[test]
    fn detect_capture_match_claude() {
        let meta = PaneMeta {
            pane_title: "random title".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            capture_lines: vec![
                "\u{256D} Claude Code".to_string(),
                "│ Working...".to_string(),
            ],
        };
        let defs = mvp_provider_defs();
        let claude_def = defs
            .iter()
            .find(|d| d.provider == Provider::Claude)
            .expect("def should exist");
        let result = detect(&meta, claude_def);

        let r = result.expect("should detect Claude via capture");
        assert_eq!(r.provider, Provider::Claude);
        assert!(r.capture_match);
        assert!(!r.cmd_match);
        assert!(!r.provider_hint);
        assert!(!r.title_match);
        assert!((r.confidence - WEIGHT_POLLER_MATCH).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_capture_match_codex() {
        let meta = PaneMeta {
            pane_title: "some title".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            capture_lines: vec!["codex> thinking about the problem".to_string()],
        };
        let defs = mvp_provider_defs();
        let codex_def = defs
            .iter()
            .find(|d| d.provider == Provider::Codex)
            .expect("codex def should exist");
        let result = detect(&meta, codex_def);

        let r = result.expect("should detect Codex via capture");
        assert!(r.capture_match);
        assert!((r.confidence - WEIGHT_POLLER_MATCH).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_stale_title_shell_suppressed() {
        // title matches "claude" but current_cmd is zsh and no capture → stale title
        let meta = PaneMeta {
            pane_title: "\u{2733} Claude Code".to_string(),
            current_cmd: "zsh".to_string(),
            process_hint: None,
            capture_lines: vec!["$ ls -la".to_string()],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "stale title + shell + no capture → None");
    }

    #[test]
    fn detect_stale_title_with_path_shell() {
        // shell as full path
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "/usr/local/bin/fish".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "stale title + path shell → None");
    }

    #[test]
    fn detect_stale_title_case_insensitive_shell() {
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "Bash".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "stale title + case-variant shell → None");
    }

    #[test]
    fn detect_title_and_capture_corroborated() {
        // Both title and capture match → detected with capture confidence
        let meta = PaneMeta {
            pane_title: "claude code".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            capture_lines: vec!["\u{256D} Claude Code".to_string()],
        };
        let result = detect_best(&meta);
        let r = result.expect("title + capture should detect");
        assert!(r.title_match);
        assert!(r.capture_match);
        assert!((r.confidence - WEIGHT_POLLER_MATCH).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_stale_title_not_suppressed_with_capture() {
        // title + shell + capture match → NOT suppressed (capture proves agent is there)
        let meta = PaneMeta {
            pane_title: "claude code".to_string(),
            current_cmd: "zsh".to_string(),
            process_hint: None,
            capture_lines: vec!["claude code is running".to_string()],
        };
        let defs = mvp_provider_defs();
        let claude_def = defs
            .iter()
            .find(|d| d.provider == Provider::Claude)
            .expect("def should exist");
        let result = detect(&meta, claude_def);
        assert!(result.is_some(), "title + shell + capture → not suppressed");
    }

    #[test]
    fn detect_cmd_basename_normalization() {
        assert_eq!(cmd_basename("zsh"), "zsh");
        assert_eq!(cmd_basename("/usr/bin/zsh"), "zsh");
        assert_eq!(cmd_basename("/usr/local/bin/fish"), "fish");
        assert_eq!(cmd_basename("Bash"), "bash");
        assert_eq!(cmd_basename("-zsh"), "zsh"); // login shell prefix stripped
        assert_eq!(cmd_basename("-bash"), "bash");
    }

    #[test]
    fn detect_stale_title_login_shell_prefix() {
        // tmux reports "-zsh" for login shells — should still be suppressed
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "-zsh".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "stale title + login shell (-zsh) → None");
    }

    #[test]
    fn known_shells_list() {
        assert!(is_known_shell("zsh"));
        assert!(is_known_shell("bash"));
        assert!(is_known_shell("fish"));
        assert!(is_known_shell("nu"));
        assert!(is_known_shell("pwsh"));
        assert!(is_known_shell("tcsh"));
        assert!(is_known_shell("csh"));
        assert!(is_known_shell("ksh"));
        assert!(is_known_shell("ash"));
        assert!(!is_known_shell("node"));
        assert!(!is_known_shell("vim"));
    }
}
