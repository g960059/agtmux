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

    // 6. Title-only suppression: title_match alone is never sufficient for detection.
    //    pane_title is unreliable (stale titles persist after process change, tmux
    //    doesn't update titles reliably). Title only counts as corroborating signal.
    if title_match && !provider_hint && !cmd_match && !capture_match {
        return None;
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

    // ── 3. Detection: title-only never sufficient ──────────────

    #[test]
    fn detect_title_only_suppressed() {
        // Title-only match is never sufficient for detection (pane_title is unreliable).
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result = detect(&meta, claude_def.expect("claude def should exist"));

        assert!(result.is_none(), "title-only match should be suppressed");
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
        // Meta that matches Claude via process_hint (1.0) and Codex via cmd (0.86)
        let meta = PaneMeta {
            pane_title: String::new(),
            current_cmd: "codex".to_string(),
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
        // Use cmd_match (not title-only) to trigger detection
        let meta = PaneMeta {
            pane_title: String::new(),
            current_cmd: "codex --model o3".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let codex_def = defs.iter().find(|d| d.provider == Provider::Codex);
        let result = detect(&meta, codex_def.expect("codex def should exist"));

        let r = result.expect("should detect Codex via cmd");
        assert!(r.is_wrapper_cmd, "Codex should have wrapper_cmd=true");

        // Claude should have wrapper_cmd=false
        let meta2 = PaneMeta {
            pane_title: String::new(),
            current_cmd: "claude".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let claude_def = defs.iter().find(|d| d.provider == Provider::Claude);
        let result2 = detect(&meta2, claude_def.expect("claude def should exist"));

        let r2 = result2.expect("should detect Claude via cmd");
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
    fn detect_case_insensitive_title_with_corroborating() {
        // Title-only is suppressed, so add a corroborating signal (cmd_match)
        let meta = PaneMeta {
            pane_title: "OPENAI CODEX".to_string(),
            current_cmd: "codex".to_string(),
            process_hint: None,
            ..Default::default()
        };
        let defs = mvp_provider_defs();
        let codex_def = defs.iter().find(|d| d.provider == Provider::Codex);
        let result = detect(&meta, codex_def.expect("codex def should exist"));

        let r = result.expect("title + cmd should detect");
        assert!(r.title_match, "case-insensitive title match should work");
        assert!(r.cmd_match);
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
    fn detect_title_only_suppressed_any_cmd() {
        // title-only match is always suppressed, regardless of current_cmd
        let meta = PaneMeta {
            pane_title: "\u{2733} Claude Code".to_string(),
            current_cmd: "node".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "title-only + node → None");

        let meta2 = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "zsh".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result2 = detect_best(&meta2);
        assert!(result2.is_none(), "title-only + zsh → None");
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
    fn detect_title_only_suppressed_login_shell() {
        // Even with login-shell prefix, title-only is still suppressed
        let meta = PaneMeta {
            pane_title: "Claude Code".to_string(),
            current_cmd: "-zsh".to_string(),
            process_hint: None,
            capture_lines: vec![],
        };
        let result = detect_best(&meta);
        assert!(result.is_none(), "title-only + login shell → None");
    }
}
