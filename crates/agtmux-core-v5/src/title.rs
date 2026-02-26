use serde::{Deserialize, Serialize};

use crate::types::Provider;

/// Title quality tier — determines which title source wins.
/// Higher-priority tiers take precedence.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum TitleQuality {
    /// Unmanaged pane fallback.
    #[default]
    Unmanaged = 0,
    /// Heuristic-only detection (pane_title match).
    HeuristicTitle = 1,
    /// Deterministic event established a session binding.
    DeterministicBinding = 2,
    /// Live pane title from tmux, confirmed by handshake.
    HandshakeConfirmed = 3,
    /// Canonical session name from provider session file.
    CanonicalSession = 4,
}

/// Title resolution decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleDecision {
    pub title: String,
    pub quality: TitleQuality,
    pub reason: String,
    pub provider: Option<Provider>,
    pub session_key: Option<String>,
}

/// Inputs for title resolution.
#[derive(Debug, Clone, Default)]
pub struct TitleInput {
    /// Current tmux pane title.
    pub pane_title: String,
    /// Provider detected for this pane (if any).
    pub provider: Option<Provider>,
    /// Session key from deterministic binding (if any).
    pub deterministic_session_key: Option<String>,
    /// Whether this pane has completed a deterministic handshake
    /// (i.e., received at least one deterministic event confirming the binding).
    pub handshake_confirmed: bool,
    /// Canonical session name from provider session file (if known).
    pub canonical_session_name: Option<String>,
    /// Whether this pane is currently managed.
    pub is_managed: bool,
}

/// Resolve the display title for a pane based on available evidence.
///
/// Priority (highest to lowest):
/// 1. `CanonicalSession` — provider session file provides authoritative name
/// 2. `HandshakeConfirmed` — pane title confirmed by deterministic handshake
/// 3. `DeterministicBinding` — deterministic event established session key
/// 4. `HeuristicTitle` — pane title from heuristic detection only
/// 5. `Unmanaged` — fallback for unmanaged panes
pub fn resolve_title(input: &TitleInput) -> TitleDecision {
    // 1. Canonical session name (highest priority)
    if let Some(ref name) = input.canonical_session_name
        && input.is_managed
    {
        return TitleDecision {
            title: name.clone(),
            quality: TitleQuality::CanonicalSession,
            reason: "canonical session name from provider session file".into(),
            provider: input.provider,
            session_key: input.deterministic_session_key.clone(),
        };
    }

    // 2. Handshake confirmed — use pane title
    if input.handshake_confirmed && !input.pane_title.is_empty() {
        return TitleDecision {
            title: input.pane_title.clone(),
            quality: TitleQuality::HandshakeConfirmed,
            reason: "pane title confirmed by deterministic handshake".into(),
            provider: input.provider,
            session_key: input.deterministic_session_key.clone(),
        };
    }

    // 3. Deterministic binding — use session key as title
    if let Some(ref session_key) = input.deterministic_session_key {
        return TitleDecision {
            title: session_key.clone(),
            quality: TitleQuality::DeterministicBinding,
            reason: "deterministic event established session binding".into(),
            provider: input.provider,
            session_key: Some(session_key.clone()),
        };
    }

    // 4. Heuristic title — provider detected and pane title available
    if input.provider.is_some() && !input.pane_title.is_empty() && input.is_managed {
        return TitleDecision {
            title: input.pane_title.clone(),
            quality: TitleQuality::HeuristicTitle,
            reason: "heuristic detection with pane title".into(),
            provider: input.provider,
            session_key: None,
        };
    }

    // 5. Unmanaged fallback
    TitleDecision {
        title: if input.pane_title.is_empty() {
            String::new()
        } else {
            input.pane_title.clone()
        },
        quality: TitleQuality::Unmanaged,
        reason: "unmanaged pane fallback".into(),
        provider: None,
        session_key: None,
    }
}

/// Format a title for status bar display (truncated to `max_len`).
///
/// Prefix with quality indicator:
/// - `"●"` for `CanonicalSession` / `HandshakeConfirmed`
/// - `"○"` for `DeterministicBinding` / `HeuristicTitle`
/// - `"·"` for `Unmanaged`
pub fn format_title_for_status(decision: &TitleDecision, max_len: usize) -> String {
    let prefix = match decision.quality {
        TitleQuality::CanonicalSession | TitleQuality::HandshakeConfirmed => "●",
        TitleQuality::DeterministicBinding | TitleQuality::HeuristicTitle => "○",
        TitleQuality::Unmanaged => "·",
    };

    if max_len == 0 {
        return String::new();
    }

    let prefix_len = prefix.chars().count();

    // If max_len can't even fit the full prefix, return truncated prefix.
    if max_len <= prefix_len {
        return prefix.chars().take(max_len).collect();
    }

    let body = &decision.title;
    let body_len = body.chars().count();
    let body_budget = max_len - prefix_len;

    if body_len <= body_budget {
        format!("{prefix}{body}")
    } else {
        // Truncate the body, preserving the prefix and adding ellipsis.
        let keep = body_budget.saturating_sub(1);
        let truncated_body: String = body.chars().take(keep).collect();
        format!("{prefix}{truncated_body}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. Canonical session wins when available ────────────────────

    #[test]
    fn canonical_session_wins_when_available() {
        let input = TitleInput {
            pane_title: "some-pane-title".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: Some("sess-001".into()),
            handshake_confirmed: true,
            canonical_session_name: Some("my-project".into()),
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::CanonicalSession);
        assert_eq!(decision.title, "my-project");
        assert_eq!(decision.provider, Some(Provider::Codex));
    }

    // ── 2. Handshake confirmed uses pane title ──────────────────────

    #[test]
    fn handshake_confirmed_uses_pane_title() {
        let input = TitleInput {
            pane_title: "Claude: my-task".into(),
            provider: Some(Provider::Claude),
            deterministic_session_key: Some("sess-002".into()),
            handshake_confirmed: true,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::HandshakeConfirmed);
        assert_eq!(decision.title, "Claude: my-task");
    }

    // ── 3. Deterministic binding uses session key ───────────────────

    #[test]
    fn deterministic_binding_uses_session_key() {
        let input = TitleInput {
            pane_title: "".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: Some("sess-003".into()),
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::DeterministicBinding);
        assert_eq!(decision.title, "sess-003");
        assert_eq!(decision.session_key, Some("sess-003".into()));
    }

    // ── 4. Heuristic title for managed pane ──────────────────────────

    #[test]
    fn heuristic_title_for_managed_pane() {
        let input = TitleInput {
            pane_title: "codex-session".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::HeuristicTitle);
        assert_eq!(decision.title, "codex-session");
    }

    // ── 5. Unmanaged fallback ────────────────────────────────────────

    #[test]
    fn unmanaged_fallback() {
        let input = TitleInput {
            pane_title: "bash".into(),
            provider: None,
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: false,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::Unmanaged);
        assert_eq!(decision.title, "bash");
        assert!(decision.provider.is_none());
    }

    // ── 6. Priority ordering ─────────────────────────────────────────

    #[test]
    fn priority_canonical_over_handshake() {
        // When both canonical_session_name and handshake_confirmed are set,
        // canonical should win.
        let input = TitleInput {
            pane_title: "pane-title".into(),
            provider: Some(Provider::Claude),
            deterministic_session_key: Some("sess-key".into()),
            handshake_confirmed: true,
            canonical_session_name: Some("canonical-name".into()),
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::CanonicalSession);
        assert_eq!(decision.title, "canonical-name");
    }

    #[test]
    fn priority_handshake_over_deterministic() {
        // When handshake_confirmed and deterministic_session_key are set
        // but no canonical, handshake should win.
        let input = TitleInput {
            pane_title: "live-title".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: Some("sess-key".into()),
            handshake_confirmed: true,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::HandshakeConfirmed);
        assert_eq!(decision.title, "live-title");
    }

    #[test]
    fn priority_deterministic_over_heuristic() {
        // When deterministic_session_key is set but handshake not confirmed
        // and no canonical, deterministic should win over heuristic.
        let input = TitleInput {
            pane_title: "heuristic-title".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: Some("sess-key".into()),
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::DeterministicBinding);
        assert_eq!(decision.title, "sess-key");
    }

    #[test]
    fn priority_heuristic_over_unmanaged() {
        // Managed pane with provider and title gets heuristic, not unmanaged.
        let input = TitleInput {
            pane_title: "some-title".into(),
            provider: Some(Provider::Claude),
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::HeuristicTitle);
    }

    // ── 7. Quality ordering via PartialOrd ──────────────────────────

    #[test]
    fn quality_ordering() {
        assert!(TitleQuality::Unmanaged < TitleQuality::HeuristicTitle);
        assert!(TitleQuality::HeuristicTitle < TitleQuality::DeterministicBinding);
        assert!(TitleQuality::DeterministicBinding < TitleQuality::HandshakeConfirmed);
        assert!(TitleQuality::HandshakeConfirmed < TitleQuality::CanonicalSession);
    }

    // ── 8. Empty pane title falls through to lower quality ──────────

    #[test]
    fn empty_pane_title_skips_handshake() {
        // handshake_confirmed but empty pane title should fall through
        let input = TitleInput {
            pane_title: "".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: Some("sess-key".into()),
            handshake_confirmed: true,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        // Should fall through to DeterministicBinding since pane_title is empty
        assert_eq!(decision.quality, TitleQuality::DeterministicBinding);
        assert_eq!(decision.title, "sess-key");
    }

    #[test]
    fn empty_pane_title_skips_heuristic() {
        // Provider detected but empty pane title should fall through to unmanaged
        let input = TitleInput {
            pane_title: "".into(),
            provider: Some(Provider::Claude),
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: true,
        };
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::Unmanaged);
        assert_eq!(decision.title, "");
    }

    #[test]
    fn unmanaged_fallback_empty_title() {
        let input = TitleInput::default();
        let decision = resolve_title(&input);
        assert_eq!(decision.quality, TitleQuality::Unmanaged);
        assert_eq!(decision.title, "");
    }

    // ── 9. Format truncation ──────────────────────────────────────────

    #[test]
    fn format_truncation() {
        let decision = TitleDecision {
            title: "a-very-long-session-name-here".into(),
            quality: TitleQuality::CanonicalSession,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        // "●" (1 char prefix) + body budget = 9 chars; body needs truncation
        // result: "●" + 8 body chars + "…" = 10 chars total
        let formatted = format_title_for_status(&decision, 10);
        assert_eq!(formatted.chars().count(), 10);
        assert!(formatted.starts_with('●'), "prefix must be preserved");
        assert!(formatted.ends_with('\u{2026}'));
    }

    #[test]
    fn format_preserves_prefix_at_tiny_max_len() {
        let decision = TitleDecision {
            title: "long-title".into(),
            quality: TitleQuality::HandshakeConfirmed,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        // max_len=1: only room for the "●" prefix character
        let formatted = format_title_for_status(&decision, 1);
        assert_eq!(formatted, "●");
    }

    #[test]
    fn format_zero_max_len_returns_empty() {
        let decision = TitleDecision {
            title: "anything".into(),
            quality: TitleQuality::CanonicalSession,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 0);
        assert!(formatted.is_empty());
    }

    #[test]
    fn format_no_truncation_within_limit() {
        let decision = TitleDecision {
            title: "ok".into(),
            quality: TitleQuality::HandshakeConfirmed,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        // "●ok" = 3 chars, max_len=10 → no truncation
        let formatted = format_title_for_status(&decision, 10);
        assert_eq!(formatted, "●ok");
        assert!(!formatted.contains('\u{2026}'));
    }

    // ── 10. Format quality indicator prefix ──────────────────────────

    #[test]
    fn format_prefix_canonical() {
        let decision = TitleDecision {
            title: "proj".into(),
            quality: TitleQuality::CanonicalSession,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 50);
        assert!(formatted.starts_with('●'));
    }

    #[test]
    fn format_prefix_handshake() {
        let decision = TitleDecision {
            title: "title".into(),
            quality: TitleQuality::HandshakeConfirmed,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 50);
        assert!(formatted.starts_with('●'));
    }

    #[test]
    fn format_prefix_deterministic() {
        let decision = TitleDecision {
            title: "sess-key".into(),
            quality: TitleQuality::DeterministicBinding,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 50);
        assert!(formatted.starts_with('○'));
    }

    #[test]
    fn format_prefix_heuristic() {
        let decision = TitleDecision {
            title: "heur".into(),
            quality: TitleQuality::HeuristicTitle,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 50);
        assert!(formatted.starts_with('○'));
    }

    #[test]
    fn format_prefix_unmanaged() {
        let decision = TitleDecision {
            title: "bash".into(),
            quality: TitleQuality::Unmanaged,
            reason: "test".into(),
            provider: None,
            session_key: None,
        };
        let formatted = format_title_for_status(&decision, 50);
        assert!(formatted.starts_with('·'));
    }

    // ── Edge: canonical requires is_managed ──────────────────────────

    #[test]
    fn canonical_requires_managed() {
        let input = TitleInput {
            pane_title: "pane".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: Some("canonical".into()),
            is_managed: false,
        };
        let decision = resolve_title(&input);
        // canonical_session_name is set but is_managed is false,
        // so it should NOT be CanonicalSession quality.
        assert_ne!(decision.quality, TitleQuality::CanonicalSession);
    }

    // ── Edge: heuristic requires is_managed ─────────────────────────

    #[test]
    fn heuristic_requires_managed() {
        let input = TitleInput {
            pane_title: "codex-session".into(),
            provider: Some(Provider::Codex),
            deterministic_session_key: None,
            handshake_confirmed: false,
            canonical_session_name: None,
            is_managed: false,
        };
        let decision = resolve_title(&input);
        // Provider and pane_title set but not managed → falls to Unmanaged
        assert_eq!(decision.quality, TitleQuality::Unmanaged);
    }

    // ── TitleQuality default ────────────────────────────────────────

    #[test]
    fn title_quality_default_is_unmanaged() {
        assert_eq!(TitleQuality::default(), TitleQuality::Unmanaged);
    }
}
