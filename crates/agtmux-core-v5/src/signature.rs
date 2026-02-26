use serde::{Deserialize, Serialize};

use crate::types::{AgtmuxError, PaneSignatureClass};

/// Heuristic signal weight: process/provider hint.
pub const WEIGHT_PROCESS_HINT: f64 = 1.00;

/// Heuristic signal weight: current_cmd token match.
pub const WEIGHT_CMD_MATCH: f64 = 0.86;

/// Heuristic signal weight: poller capture signal.
pub const WEIGHT_POLLER_MATCH: f64 = 0.78;

/// Heuristic signal weight: pane_title token match.
pub const WEIGHT_TITLE_MATCH: f64 = 0.66;

/// Hysteresis: idle confirmation window minimum (seconds).
pub const HYSTERESIS_IDLE_MIN_SECS: u64 = 4;

/// Hysteresis: running promotion window (seconds).
pub const HYSTERESIS_RUNNING_PROMOTE_SECS: u64 = 8;

/// Hysteresis: running demotion window (seconds).
pub const HYSTERESIS_RUNNING_DEMOTE_SECS: u64 = 45;

/// Hysteresis: no-agent streak threshold for unmanaged demotion.
pub const NO_AGENT_DEMOTION_STREAK: u32 = 2;

/// Inputs for pane signature classification.
/// The first four fields are exposed in the client API; internal fields are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureInputs {
    pub provider_hint: bool,
    pub cmd_match: bool,
    pub poller_match: bool,
    pub title_match: bool,
    #[serde(skip)]
    pub has_deterministic_fields: bool,
    #[serde(skip)]
    pub is_wrapper_cmd: bool,
    #[serde(skip)]
    pub no_agent_streak: u32,
    /// Set by the caller when deterministic classification is expected
    /// (e.g. a deterministic source was previously active for this pane).
    /// When true and `has_deterministic_fields` is false with no heuristic
    /// signals, returns `Err(SignatureInconclusive)` instead of `Ok(None)`.
    #[serde(skip)]
    pub deterministic_expected: bool,
    /// Set by the caller when deterministic evidence is fresh for this pane.
    /// FR-028: while deterministic is fresh, no-agent demotion must NOT fire.
    #[serde(skip)]
    pub deterministic_fresh_active: bool,
}

/// Result of pane signature classification.
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureResult {
    pub class: PaneSignatureClass,
    pub reason: String,
    pub confidence: f64,
    /// Set when the only matching signal is title_match.
    /// Title-only matches must NOT trigger managed promotion.
    pub title_only_guard: bool,
}

/// Pure classifier: determines pane signature class from available signals.
///
/// # Rules (v1)
///
/// 1. **Deterministic**: If `has_deterministic_fields` is set, all required fields
///    (`provider, source_kind, pane_instance_id, session_key, source_event_id, event_time`)
///    are present. Returns `Deterministic` with confidence 1.0.
///
/// 2. **Inconclusive**: If `deterministic_expected` is set but `has_deterministic_fields`
///    is false and no heuristic signals match, returns `Err(SignatureInconclusive)`.
///
/// 3. **No-agent demotion**: If `no_agent_streak >= NO_AGENT_DEMOTION_STREAK` and
///    the classification would otherwise be heuristic, demote to `None`.
///    **Exception (FR-028)**: if `deterministic_fresh_active` is set, demotion is skipped.
///
/// 4. **Heuristic scoring**: Confidence is the maximum weight among matched signals.
///
/// 5. **Guardrails**:
///    - Title-only match sets `title_only_guard = true` (must not trigger managed promotion).
///    - Wrapper command (`is_wrapper_cmd`) + no provider hint + title-only → reject.
///
/// 6. **None**: No signals matched → `None` with confidence 0.0.
pub fn classify(inputs: &SignatureInputs) -> Result<SignatureResult, AgtmuxError> {
    // ── Deterministic path ──────────────────────────────────────────
    if inputs.has_deterministic_fields {
        // All required fields present: deterministic classification.
        return Ok(SignatureResult {
            class: PaneSignatureClass::Deterministic,
            reason: "all required deterministic fields present".into(),
            confidence: 1.0,
            title_only_guard: false,
        });
    }

    // ── Collect heuristic signals ───────────────────────────────────
    let mut max_weight: f64 = 0.0;
    let mut reasons: Vec<&str> = Vec::new();
    let mut has_any_signal = false;

    if inputs.provider_hint {
        max_weight = f64_max(max_weight, WEIGHT_PROCESS_HINT);
        reasons.push("provider_hint");
        has_any_signal = true;
    }
    if inputs.cmd_match {
        max_weight = f64_max(max_weight, WEIGHT_CMD_MATCH);
        reasons.push("cmd_match");
        has_any_signal = true;
    }
    if inputs.poller_match {
        max_weight = f64_max(max_weight, WEIGHT_POLLER_MATCH);
        reasons.push("poller_match");
        has_any_signal = true;
    }
    if inputs.title_match {
        max_weight = f64_max(max_weight, WEIGHT_TITLE_MATCH);
        reasons.push("title_match");
        has_any_signal = true;
    }

    // ── No signals at all ───────────────────────────────────────────
    if !has_any_signal {
        // If deterministic was expected but fields are missing and no heuristic
        // signals are available, this is an inconclusive classification.
        if inputs.deterministic_expected {
            return Err(AgtmuxError::SignatureInconclusive);
        }
        return Ok(SignatureResult {
            class: PaneSignatureClass::None,
            reason: "no heuristic signals matched".into(),
            confidence: 0.0,
            title_only_guard: false,
        });
    }

    // ── Guardrail: title-only detection ─────────────────────────────
    let title_only =
        inputs.title_match && !inputs.provider_hint && !inputs.cmd_match && !inputs.poller_match;

    // Guardrail: wrapper command + no provider hint + title-only → reject
    if title_only && inputs.is_wrapper_cmd {
        return Err(AgtmuxError::SignatureGuardRejected(
            "wrapper command (node|bun|deno) with title-only match and no provider hint".into(),
        ));
    }

    // ── No-agent demotion ───────────────────────────────────────────
    // FR-028: while deterministic evidence is fresh, no-agent demotion must
    // NOT fire — the managed state is sustained by deterministic presence.
    if inputs.no_agent_streak >= NO_AGENT_DEMOTION_STREAK && !inputs.deterministic_fresh_active {
        return Ok(SignatureResult {
            class: PaneSignatureClass::None,
            reason: format!(
                "no-agent streak ({}) >= threshold ({}); demoted",
                inputs.no_agent_streak, NO_AGENT_DEMOTION_STREAK
            ),
            confidence: 0.0,
            title_only_guard: false,
        });
    }

    // ── Heuristic classification ────────────────────────────────────
    let reason = reasons.join("+");
    Ok(SignatureResult {
        class: PaneSignatureClass::Heuristic,
        reason: format!("heuristic({reason})"),
        confidence: max_weight,
        title_only_guard: title_only,
    })
}

/// f64 does not implement Ord, so we provide a simple max helper.
fn f64_max(a: f64, b: f64) -> f64 {
    if a >= b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_descending_order() {
        // Validated at compile time: process_hint >= cmd >= poller >= title
        const {
            assert!(WEIGHT_PROCESS_HINT >= WEIGHT_CMD_MATCH);
            assert!(WEIGHT_CMD_MATCH >= WEIGHT_POLLER_MATCH);
            assert!(WEIGHT_POLLER_MATCH >= WEIGHT_TITLE_MATCH);
        };
    }

    #[test]
    fn signature_inputs_default() {
        let inputs = SignatureInputs::default();
        assert!(!inputs.provider_hint);
        assert!(!inputs.cmd_match);
        assert!(!inputs.poller_match);
        assert!(!inputs.title_match);
    }

    #[test]
    fn signature_inputs_serde_skips_internal_fields() {
        let inputs = SignatureInputs {
            provider_hint: true,
            has_deterministic_fields: true,
            is_wrapper_cmd: true,
            no_agent_streak: 5,
            ..Default::default()
        };
        let json = serde_json::to_string(&inputs).expect("serialize");
        assert!(!json.contains("has_deterministic_fields"));
        assert!(!json.contains("is_wrapper_cmd"));
        assert!(!json.contains("no_agent_streak"));
        assert!(json.contains("provider_hint"));
    }

    // ── Deterministic path ──────────────────────────────────────────

    #[test]
    fn deterministic_all_required_fields_present() {
        let inputs = SignatureInputs {
            has_deterministic_fields: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Deterministic);
        assert!((result.confidence - 1.0).abs() < f64::EPSILON);
        assert!(!result.title_only_guard);
    }

    #[test]
    fn deterministic_missing_field_returns_inconclusive_via_none() {
        // When has_deterministic_fields is false and no heuristic signals,
        // classify returns None (the caller treats this as inconclusive
        // if deterministic was expected).
        let inputs = SignatureInputs::default();
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::None);
        assert!((result.confidence - 0.0).abs() < f64::EPSILON);
    }

    // ── Heuristic: process/provider hint ────────────────────────────

    #[test]
    fn heuristic_process_hint() {
        let inputs = SignatureInputs {
            provider_hint: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
        assert!(
            (result.confidence - WEIGHT_PROCESS_HINT).abs() < f64::EPSILON,
            "expected confidence {WEIGHT_PROCESS_HINT}, got {}",
            result.confidence,
        );
        assert!(!result.title_only_guard);
    }

    // ── Heuristic: cmd_match only ───────────────────────────────────

    #[test]
    fn heuristic_cmd_match_only() {
        let inputs = SignatureInputs {
            cmd_match: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
        assert!(
            (result.confidence - WEIGHT_CMD_MATCH).abs() < f64::EPSILON,
            "expected confidence {WEIGHT_CMD_MATCH}, got {}",
            result.confidence,
        );
    }

    // ── Heuristic: poller_match only ────────────────────────────────

    #[test]
    fn heuristic_poller_match_only() {
        let inputs = SignatureInputs {
            poller_match: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
        assert!(
            (result.confidence - WEIGHT_POLLER_MATCH).abs() < f64::EPSILON,
            "expected confidence {WEIGHT_POLLER_MATCH}, got {}",
            result.confidence,
        );
    }

    // ── Heuristic: title_match only (not wrapper) ───────────────────

    #[test]
    fn heuristic_title_match_only_not_wrapper() {
        let inputs = SignatureInputs {
            title_match: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
        assert!(
            (result.confidence - WEIGHT_TITLE_MATCH).abs() < f64::EPSILON,
            "expected confidence {WEIGHT_TITLE_MATCH}, got {}",
            result.confidence,
        );
        assert!(
            result.title_only_guard,
            "title-only match must set title_only_guard"
        );
    }

    // ── Guardrail: title-only sets title_only_guard ─────────────────

    #[test]
    fn guardrail_title_only_sets_guard_flag() {
        let inputs = SignatureInputs {
            title_match: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert!(
            result.title_only_guard,
            "title-only match must set title_only_guard to prevent managed promotion"
        );
    }

    // ── Guardrail: title-only + provider_hint → NOT title-only ──────

    #[test]
    fn guardrail_title_with_provider_hint_not_title_only() {
        let inputs = SignatureInputs {
            title_match: true,
            provider_hint: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert!(!result.title_only_guard);
    }

    // ── Guardrail: wrapper + no provider + title-only → rejected ────

    #[test]
    fn guardrail_wrapper_cmd_title_only_rejected() {
        let inputs = SignatureInputs {
            title_match: true,
            is_wrapper_cmd: true,
            ..Default::default()
        };
        let err = classify(&inputs).expect_err("should be rejected");
        match err {
            AgtmuxError::SignatureGuardRejected(msg) => {
                assert!(
                    msg.contains("wrapper"),
                    "rejection reason should mention wrapper, got: {msg}"
                );
            }
            other => panic!("expected SignatureGuardRejected, got: {other:?}"),
        }
    }

    // ── Guardrail: wrapper + provider hint → NOT rejected ───────────

    #[test]
    fn guardrail_wrapper_cmd_with_provider_hint_ok() {
        let inputs = SignatureInputs {
            title_match: true,
            provider_hint: true,
            is_wrapper_cmd: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed with provider hint");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
    }

    // ── No signals → None ───────────────────────────────────────────

    #[test]
    fn no_signals_returns_none() {
        let inputs = SignatureInputs::default();
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::None);
        assert!((result.confidence - 0.0).abs() < f64::EPSILON);
        assert!(!result.title_only_guard);
    }

    // ── No-agent demotion ───────────────────────────────────────────

    #[test]
    fn no_agent_demotion_at_streak_threshold() {
        let inputs = SignatureInputs {
            provider_hint: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(
            result.class,
            PaneSignatureClass::None,
            "streak at threshold should demote to None"
        );
        assert!((result.confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_agent_demotion_above_streak_threshold() {
        let inputs = SignatureInputs {
            cmd_match: true,
            poller_match: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK + 1,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::None);
    }

    #[test]
    fn no_agent_streak_below_threshold_allows_heuristic() {
        let inputs = SignatureInputs {
            provider_hint: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK - 1,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
    }

    // ── No-agent demotion does NOT affect deterministic ──────────────

    #[test]
    fn no_agent_demotion_does_not_affect_deterministic() {
        let inputs = SignatureInputs {
            has_deterministic_fields: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK + 10,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(
            result.class,
            PaneSignatureClass::Deterministic,
            "deterministic path is not affected by no-agent streak"
        );
    }

    // ── Combined signals → max weight ───────────────────────────────

    #[test]
    fn combined_signals_use_max_weight() {
        let inputs = SignatureInputs {
            cmd_match: true,
            poller_match: true,
            title_match: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
        assert!(
            (result.confidence - WEIGHT_CMD_MATCH).abs() < f64::EPSILON,
            "confidence should be max weight (cmd_match = {WEIGHT_CMD_MATCH}), got {}",
            result.confidence,
        );
        assert!(
            !result.title_only_guard,
            "not title-only when other signals present"
        );
    }

    // ── Weight ordering is maintained ───────────────────────────────

    #[test]
    fn weight_ordering_maintained() {
        // Strict ordering validated at compile time.
        const {
            assert!(WEIGHT_PROCESS_HINT > WEIGHT_CMD_MATCH);
            assert!(WEIGHT_CMD_MATCH > WEIGHT_POLLER_MATCH);
            assert!(WEIGHT_POLLER_MATCH > WEIGHT_TITLE_MATCH);
            assert!(WEIGHT_TITLE_MATCH > 0.0);
        };
    }

    // ── Deterministic expected → inconclusive ─────────────────────

    #[test]
    fn deterministic_expected_but_missing_returns_inconclusive() {
        let inputs = SignatureInputs {
            deterministic_expected: true,
            ..Default::default()
        };
        let err = classify(&inputs).expect_err("should return inconclusive");
        assert_eq!(err, AgtmuxError::SignatureInconclusive);
    }

    #[test]
    fn deterministic_expected_with_heuristic_signals_still_ok() {
        // When deterministic is expected but missing, heuristic signals
        // provide a fallback classification.
        let inputs = SignatureInputs {
            deterministic_expected: true,
            provider_hint: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed with heuristic fallback");
        assert_eq!(result.class, PaneSignatureClass::Heuristic);
    }

    #[test]
    fn deterministic_expected_and_present_returns_deterministic() {
        let inputs = SignatureInputs {
            deterministic_expected: true,
            has_deterministic_fields: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(result.class, PaneSignatureClass::Deterministic);
    }

    // ── FR-028: deterministic_fresh_active skips no-agent demotion ─

    #[test]
    fn deterministic_fresh_active_prevents_no_agent_demotion() {
        let inputs = SignatureInputs {
            provider_hint: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK + 5,
            deterministic_fresh_active: true,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(
            result.class,
            PaneSignatureClass::Heuristic,
            "demotion must be skipped when deterministic is fresh"
        );
    }

    #[test]
    fn deterministic_fresh_active_false_allows_demotion() {
        let inputs = SignatureInputs {
            provider_hint: true,
            no_agent_streak: NO_AGENT_DEMOTION_STREAK,
            deterministic_fresh_active: false,
            ..Default::default()
        };
        let result = classify(&inputs).expect("should succeed");
        assert_eq!(
            result.class,
            PaneSignatureClass::None,
            "demotion should fire when deterministic is not fresh"
        );
    }

    // ── Serde: new internal fields also skipped ───────────────────

    #[test]
    fn signature_inputs_serde_skips_new_internal_fields() {
        let inputs = SignatureInputs {
            provider_hint: true,
            deterministic_expected: true,
            deterministic_fresh_active: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&inputs).expect("serialize");
        assert!(!json.contains("deterministic_expected"));
        assert!(!json.contains("deterministic_fresh_active"));
    }
}
