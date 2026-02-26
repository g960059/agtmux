use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{HashMap, HashSet};

use crate::types::{EvidenceTier, Provider, SourceEventV2, SourceKind};

/// Freshness threshold for deterministic source (seconds).
pub const FRESH_THRESHOLD_SECS: u64 = 3;

/// Down threshold for deterministic source (seconds).
pub const DOWN_THRESHOLD_SECS: u64 = 15;

/// Source rank entry for per-provider priority ordering.
/// Lower rank value = higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRank {
    pub provider: Provider,
    pub source_kind: SourceKind,
    pub rank: u32,
}

/// Default source rank policy (MVP).
///
/// - Codex: appserver (rank 0) > poller (rank 1)
/// - Claude: hooks (rank 0) > poller (rank 1)
pub fn default_source_ranks() -> Vec<SourceRank> {
    vec![
        SourceRank {
            provider: Provider::Codex,
            source_kind: SourceKind::CodexAppserver,
            rank: 0,
        },
        SourceRank {
            provider: Provider::Codex,
            source_kind: SourceKind::Poller,
            rank: 1,
        },
        SourceRank {
            provider: Provider::Claude,
            source_kind: SourceKind::ClaudeHooks,
            rank: 0,
        },
        SourceRank {
            provider: Provider::Claude,
            source_kind: SourceKind::Poller,
            rank: 1,
        },
    ]
}

// ─── Resolver State & Output ─────────────────────────────────────

/// Persistent state carried across resolver invocations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverState {
    pub current_tier: EvidenceTier,
    pub deterministic_last_seen: Option<DateTime<Utc>>,
}

impl Default for ResolverState {
    fn default() -> Self {
        Self {
            current_tier: EvidenceTier::Heuristic,
            deterministic_last_seen: None,
        }
    }
}

/// Result of tier winner resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverResult {
    pub winner_tier: EvidenceTier,
    pub is_fallback: bool,
    pub re_promoted: bool,
}

/// Full output of the resolver, including event disposition.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolverOutput {
    pub result: ResolverResult,
    pub accepted_events: Vec<SourceEventV2>,
    pub suppressed_events: Vec<SourceEventV2>,
    pub duplicates_dropped: usize,
    /// Updated state to carry forward into the next invocation.
    pub next_state: ResolverState,
}

// ─── Freshness ───────────────────────────────────────────────────

/// Freshness classification for a deterministic source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Down,
}

/// Classify freshness based on elapsed time since last deterministic event.
pub fn classify_freshness(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Freshness {
    match last_seen {
        None => Freshness::Down,
        Some(ts) => {
            let elapsed = now.signed_duration_since(ts);
            let fresh_limit = TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64);
            let down_limit = TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64);
            if elapsed <= fresh_limit {
                Freshness::Fresh
            } else if elapsed <= down_limit {
                Freshness::Stale
            } else {
                Freshness::Down
            }
        }
    }
}

// ─── Dedup ───────────────────────────────────────────────────────

/// Dedup key: (provider, session_key, event_id).
type DedupKey = (Provider, String, String);

fn dedup_key(event: &SourceEventV2) -> DedupKey {
    (
        event.provider,
        event.session_key.clone(),
        event.event_id.clone(),
    )
}

/// Remove duplicate events, keeping the first occurrence.
/// Returns (unique events, number of duplicates dropped).
fn dedup_events(events: Vec<SourceEventV2>) -> (Vec<SourceEventV2>, usize) {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(events.len());
    let mut dropped = 0usize;

    for event in events {
        let key = dedup_key(&event);
        if seen.contains(&key) {
            dropped += 1;
        } else {
            seen.insert(key);
            unique.push(event);
        }
    }

    (unique, dropped)
}

// ─── Source Rank Suppression ─────────────────────────────────────

/// Build a lookup map from (provider, source_kind) -> rank.
fn build_rank_map(ranks: &[SourceRank]) -> HashMap<(Provider, SourceKind), u32> {
    ranks
        .iter()
        .map(|r| ((r.provider, r.source_kind), r.rank))
        .collect()
}

/// Within each provider, suppress lower-ranked sources if a higher-ranked
/// source is present in the same batch.
///
/// Returns (accepted, suppressed).
fn apply_source_rank(
    events: Vec<SourceEventV2>,
    rank_map: &HashMap<(Provider, SourceKind), u32>,
) -> (Vec<SourceEventV2>, Vec<SourceEventV2>) {
    // For each provider, find the best (lowest) rank present in the batch.
    let mut best_rank_per_provider: HashMap<Provider, u32> = HashMap::new();
    for event in &events {
        let rank = rank_map
            .get(&(event.provider, event.source_kind))
            .copied()
            .unwrap_or(u32::MAX);
        let entry = best_rank_per_provider.entry(event.provider).or_insert(rank);
        if rank < *entry {
            *entry = rank;
        }
    }

    let mut accepted = Vec::new();
    let mut suppressed = Vec::new();

    for event in events {
        let event_rank = rank_map
            .get(&(event.provider, event.source_kind))
            .copied()
            .unwrap_or(u32::MAX);
        let best = best_rank_per_provider
            .get(&event.provider)
            .copied()
            .unwrap_or(u32::MAX);

        if event_rank <= best {
            accepted.push(event);
        } else {
            suppressed.push(event);
        }
    }

    (accepted, suppressed)
}

// ─── Tier Resolution (main entry point) ──────────────────────────

/// Resolve the winner tier from a batch of source events for a **single session**.
///
/// This is a **pure function**: no IO, no global state. All needed context
/// is supplied through the arguments. The daemon calls this function once per
/// session per tick; cross-session batching is the caller's responsibility.
///
/// # Arguments
/// * `events` - Batch of source events to resolve (single session).
/// * `now` - Current wall-clock time for freshness evaluation.
/// * `prev_state` - Optional previous resolver state (for re-promotion detection).
/// * `source_ranks` - Source rank policy (use `default_source_ranks()` for MVP).
pub fn resolve(
    events: Vec<SourceEventV2>,
    now: DateTime<Utc>,
    prev_state: Option<&ResolverState>,
    source_ranks: &[SourceRank],
) -> ResolverOutput {
    // Step 1: Dedup by (provider, session_key, event_id)
    let (unique_events, duplicates_dropped) = dedup_events(events);

    // Step 2: Compute latest deterministic observation time from ALL unique events
    // (before rank suppression, so stale det presence is detected even when the
    // poller is also present in the batch).
    let batch_det_latest: Option<DateTime<Utc>> = unique_events
        .iter()
        .filter(|e| e.tier == EvidenceTier::Deterministic)
        .map(|e| e.observed_at)
        .max();

    // Merge with previously tracked deterministic last-seen
    let det_last_seen = match (
        prev_state.and_then(|s| s.deterministic_last_seen),
        batch_det_latest,
    ) {
        (Some(prev), Some(batch)) => Some(std::cmp::max(prev, batch)),
        (a, b) => a.or(b),
    };

    // Step 3: Freshness classification
    let freshness = classify_freshness(det_last_seen, now);

    // Step 4: Winner tier selection
    let (winner_tier, is_fallback) = match freshness {
        Freshness::Fresh => (EvidenceTier::Deterministic, false),
        Freshness::Stale | Freshness::Down => (EvidenceTier::Heuristic, true),
    };

    // Step 5: Re-promotion detection
    // Re-promotion only occurs when there was an explicit previous state in the
    // Heuristic tier and we are now switching back to Deterministic. The very
    // first invocation (no prev_state) is an initial promotion, not a re-promotion.
    let re_promoted = prev_state.is_some_and(|s| {
        s.current_tier == EvidenceTier::Heuristic && winner_tier == EvidenceTier::Deterministic
    });

    // Step 6: Partition by tier → keep winner-tier events, suppress the rest
    let mut winner_tier_events = Vec::new();
    let mut tier_suppressed = Vec::new();
    for event in unique_events {
        if event.tier == winner_tier {
            winner_tier_events.push(event);
        } else {
            tier_suppressed.push(event);
        }
    }

    // Step 7: Source rank suppression WITHIN the winner tier only.
    // This ensures that when det is stale and winner is heuristic, poller events
    // are not incorrectly rank-suppressed by the (stale) deterministic source.
    let rank_map = build_rank_map(source_ranks);
    let (accepted_events, rank_suppressed) = apply_source_rank(winner_tier_events, &rank_map);

    // Combine tier-suppressed and rank-suppressed
    let mut suppressed_events = tier_suppressed;
    suppressed_events.extend(rank_suppressed);

    // Step 8: Build next state
    let next_state = ResolverState {
        current_tier: winner_tier,
        deterministic_last_seen: det_last_seen,
    };

    ResolverOutput {
        result: ResolverResult {
            winner_tier,
            is_fallback,
            re_promoted,
        },
        accepted_events,
        suppressed_events,
        duplicates_dropped,
        next_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    // ─── Test Helpers ────────────────────────────────────────────

    fn make_event(
        event_id: &str,
        provider: Provider,
        source_kind: SourceKind,
        session_key: &str,
        observed_at: DateTime<Utc>,
    ) -> SourceEventV2 {
        SourceEventV2 {
            event_id: event_id.to_string(),
            provider,
            source_kind,
            tier: source_kind.tier(),
            observed_at,
            session_key: session_key.to_string(),
            pane_id: Some("%1".to_string()),
            pane_generation: Some(1),
            pane_birth_ts: Some(observed_at),
            source_event_id: Some(format!("src-{event_id}")),
            event_type: "lifecycle.running".to_string(),
            payload: serde_json::json!({"status": "running"}),
            confidence: 1.0,
        }
    }

    fn ranks() -> Vec<SourceRank> {
        default_source_ranks()
    }

    // ─── Existing Tests (preserved from T-010) ───────────────────

    #[test]
    fn default_ranks_codex_appserver_highest() {
        let ranks = default_source_ranks();
        let codex_app = ranks
            .iter()
            .find(|r| r.source_kind == SourceKind::CodexAppserver)
            .expect("codex appserver rank");
        let codex_poll = ranks
            .iter()
            .find(|r| r.provider == Provider::Codex && r.source_kind == SourceKind::Poller)
            .expect("codex poller rank");
        assert!(codex_app.rank < codex_poll.rank);
    }

    #[test]
    fn default_ranks_claude_hooks_highest() {
        let ranks = default_source_ranks();
        let claude_hooks = ranks
            .iter()
            .find(|r| r.source_kind == SourceKind::ClaudeHooks)
            .expect("claude hooks rank");
        let claude_poll = ranks
            .iter()
            .find(|r| r.provider == Provider::Claude && r.source_kind == SourceKind::Poller)
            .expect("claude poller rank");
        assert!(claude_hooks.rank < claude_poll.rank);
    }

    #[test]
    fn freshness_thresholds() {
        const { assert!(FRESH_THRESHOLD_SECS < DOWN_THRESHOLD_SECS) };
    }

    // ─── Freshness Classification ────────────────────────────────

    #[test]
    fn classify_freshness_no_last_seen_is_down() {
        let now = Utc::now();
        assert_eq!(classify_freshness(None, now), Freshness::Down);
    }

    #[test]
    fn classify_freshness_within_threshold_is_fresh() {
        let now = Utc::now();
        let last = now - TimeDelta::seconds(2);
        assert_eq!(classify_freshness(Some(last), now), Freshness::Fresh);
    }

    #[test]
    fn classify_freshness_at_threshold_boundary_is_fresh() {
        let now = Utc::now();
        let last = now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64);
        assert_eq!(classify_freshness(Some(last), now), Freshness::Fresh);
    }

    #[test]
    fn classify_freshness_beyond_fresh_is_stale() {
        let now = Utc::now();
        let last = now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 1);
        assert_eq!(classify_freshness(Some(last), now), Freshness::Stale);
    }

    #[test]
    fn classify_freshness_at_down_threshold_is_stale() {
        let now = Utc::now();
        let last = now - TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64);
        assert_eq!(classify_freshness(Some(last), now), Freshness::Stale);
    }

    #[test]
    fn classify_freshness_beyond_down_threshold_is_down() {
        let now = Utc::now();
        let last = now - TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64 + 1);
        assert_eq!(classify_freshness(Some(last), now), Freshness::Down);
    }

    // ─── Winner: Fresh Deterministic ─────────────────────────────

    #[test]
    fn fresh_deterministic_wins() {
        let now = Utc::now();
        let events = vec![make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now - TimeDelta::seconds(1),
        )];

        let output = resolve(events, now, None, &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Deterministic,
            "fresh deterministic event should win"
        );
        assert!(!output.result.is_fallback);
        assert_eq!(output.accepted_events.len(), 1);
        assert_eq!(output.duplicates_dropped, 0);
    }

    // ─── Winner: Stale Deterministic -> Fallback ─────────────────

    #[test]
    fn stale_deterministic_falls_back_to_heuristic() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(
                now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 2),
            ),
        };
        let heur_event = make_event(
            "evt-heur",
            Provider::Codex,
            SourceKind::Poller,
            "sess-1",
            now - TimeDelta::seconds(1),
        );

        let output = resolve(vec![heur_event], now, Some(&prev), &ranks());

        assert_eq!(output.result.winner_tier, EvidenceTier::Heuristic);
        assert!(output.result.is_fallback);
        assert!(!output.result.re_promoted);
    }

    // ─── Winner: Down Deterministic -> Fallback ──────────────────

    #[test]
    fn down_deterministic_falls_back_to_heuristic() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(now - TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64 + 5)),
        };
        let heur_event = make_event(
            "evt-heur",
            Provider::Claude,
            SourceKind::Poller,
            "sess-1",
            now - TimeDelta::seconds(1),
        );

        let output = resolve(vec![heur_event], now, Some(&prev), &ranks());

        assert_eq!(output.result.winner_tier, EvidenceTier::Heuristic);
        assert!(output.result.is_fallback);
        assert!(!output.result.re_promoted);
    }

    // ─── Re-Promotion ────────────────────────────────────────────

    #[test]
    fn re_promotion_from_heuristic_to_deterministic() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Heuristic,
            deterministic_last_seen: Some(
                now - TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64 + 10),
            ),
        };

        let det_event = make_event(
            "evt-det-fresh",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now - TimeDelta::seconds(1),
        );

        let output = resolve(vec![det_event], now, Some(&prev), &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Deterministic,
            "should re-promote to deterministic"
        );
        assert!(!output.result.is_fallback);
        assert!(output.result.re_promoted, "re_promoted flag should be true");
        assert_eq!(
            output.next_state.current_tier,
            EvidenceTier::Deterministic,
            "next state should reflect deterministic"
        );
    }

    #[test]
    fn no_re_promotion_when_already_deterministic() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(now - TimeDelta::seconds(1)),
        };
        let det_event = make_event(
            "evt-det",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );

        let output = resolve(vec![det_event], now, Some(&prev), &ranks());

        assert_eq!(output.result.winner_tier, EvidenceTier::Deterministic);
        assert!(
            !output.result.re_promoted,
            "should not re-promote when already deterministic"
        );
    }

    // ─── Dedup ───────────────────────────────────────────────────

    #[test]
    fn dedup_drops_duplicate_events() {
        let now = Utc::now();
        let evt1 = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );
        let evt1_dup = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );
        let evt2 = make_event(
            "evt-2",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );

        let events = vec![evt1, evt1_dup, evt2];
        let output = resolve(events, now, None, &ranks());

        assert_eq!(output.duplicates_dropped, 1, "should drop 1 duplicate");
        assert_eq!(output.accepted_events.len(), 2);
    }

    #[test]
    fn dedup_same_event_id_different_provider_not_duplicate() {
        let now = Utc::now();
        let evt_codex = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );
        let evt_claude = make_event(
            "evt-1",
            Provider::Claude,
            SourceKind::ClaudeHooks,
            "sess-1",
            now,
        );

        let events = vec![evt_codex, evt_claude];
        let output = resolve(events, now, None, &ranks());

        assert_eq!(
            output.duplicates_dropped, 0,
            "different providers are not duplicates"
        );
        assert_eq!(output.accepted_events.len(), 2);
    }

    #[test]
    fn dedup_same_event_id_different_session_not_duplicate() {
        let now = Utc::now();
        let evt_s1 = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );
        let evt_s2 = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-2",
            now,
        );

        let events = vec![evt_s1, evt_s2];
        let output = resolve(events, now, None, &ranks());

        assert_eq!(
            output.duplicates_dropped, 0,
            "different sessions are not duplicates"
        );
    }

    // ─── Source Rank: Codex appserver suppresses poller ───────────

    #[test]
    fn source_rank_codex_appserver_suppresses_poller() {
        let now = Utc::now();
        let appserver_evt = make_event(
            "evt-app",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            now,
        );
        let poller_evt = make_event(
            "evt-poll",
            Provider::Codex,
            SourceKind::Poller,
            "sess-1",
            now,
        );

        let events = vec![appserver_evt, poller_evt];
        let output = resolve(events, now, None, &ranks());

        assert_eq!(
            output.suppressed_events.len(),
            1,
            "poller should be suppressed"
        );
        assert_eq!(
            output.suppressed_events[0].source_kind,
            SourceKind::Poller,
            "suppressed event should be the poller"
        );
        assert_eq!(
            output.accepted_events.len(),
            1,
            "only appserver event accepted"
        );
        assert_eq!(
            output.accepted_events[0].source_kind,
            SourceKind::CodexAppserver,
        );
    }

    // ─── Source Rank: Claude hooks suppresses poller ──────────────

    #[test]
    fn source_rank_claude_hooks_suppresses_poller() {
        let now = Utc::now();
        let hooks_evt = make_event(
            "evt-hooks",
            Provider::Claude,
            SourceKind::ClaudeHooks,
            "sess-1",
            now,
        );
        let poller_evt = make_event(
            "evt-poll",
            Provider::Claude,
            SourceKind::Poller,
            "sess-1",
            now,
        );

        let events = vec![hooks_evt, poller_evt];
        let output = resolve(events, now, None, &ranks());

        assert_eq!(output.suppressed_events.len(), 1);
        assert_eq!(output.suppressed_events[0].source_kind, SourceKind::Poller,);
        assert_eq!(output.accepted_events.len(), 1);
        assert_eq!(
            output.accepted_events[0].source_kind,
            SourceKind::ClaudeHooks,
        );
    }

    // ─── Mixed Providers: Independent Resolution ─────────────────

    #[test]
    fn mixed_providers_resolve_independently() {
        let now = Utc::now();
        let codex_app = make_event(
            "evt-codex-app",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-codex",
            now,
        );
        let codex_poll = make_event(
            "evt-codex-poll",
            Provider::Codex,
            SourceKind::Poller,
            "sess-codex",
            now,
        );
        let claude_hooks = make_event(
            "evt-claude-hooks",
            Provider::Claude,
            SourceKind::ClaudeHooks,
            "sess-claude",
            now,
        );
        let claude_poll = make_event(
            "evt-claude-poll",
            Provider::Claude,
            SourceKind::Poller,
            "sess-claude",
            now,
        );

        let events = vec![codex_app, codex_poll, claude_hooks, claude_poll];
        let output = resolve(events, now, None, &ranks());

        // Both pollers suppressed by rank
        assert_eq!(
            output.suppressed_events.len(),
            2,
            "both pollers should be suppressed"
        );
        // Both deterministic events accepted (fresh)
        assert_eq!(
            output.accepted_events.len(),
            2,
            "both deterministic events accepted"
        );
        assert_eq!(output.result.winner_tier, EvidenceTier::Deterministic);
        assert!(!output.result.is_fallback);
    }

    #[test]
    fn mixed_providers_poller_only_claude_with_fresh_codex() {
        let now = Utc::now();
        let codex_app = make_event(
            "evt-codex-app",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-codex",
            now - TimeDelta::seconds(1),
        );
        // Claude: only poller (no hooks) -- not rank-suppressed
        let claude_poll = make_event(
            "evt-claude-poll",
            Provider::Claude,
            SourceKind::Poller,
            "sess-claude",
            now - TimeDelta::seconds(1),
        );

        let events = vec![codex_app, claude_poll];
        let output = resolve(events, now, None, &ranks());

        // Codex appserver is fresh deterministic -> winner is Deterministic
        assert_eq!(output.result.winner_tier, EvidenceTier::Deterministic);
        // Accepted: only codex_app (deterministic tier wins)
        assert_eq!(output.accepted_events.len(), 1);
        assert_eq!(
            output.accepted_events[0].source_kind,
            SourceKind::CodexAppserver,
        );
        // claude_poll: tier-suppressed (heuristic in a deterministic-winning batch)
        assert_eq!(output.suppressed_events.len(), 1);
        assert_eq!(output.suppressed_events[0].source_kind, SourceKind::Poller,);
    }

    // ─── Edge Cases ──────────────────────────────────────────────

    #[test]
    fn empty_batch_with_no_prior_state() {
        let now = Utc::now();
        let output = resolve(vec![], now, None, &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Heuristic,
            "no events and no prior state -> heuristic fallback"
        );
        assert!(output.result.is_fallback);
        assert!(!output.result.re_promoted);
        assert!(output.accepted_events.is_empty());
        assert!(output.suppressed_events.is_empty());
        assert_eq!(output.duplicates_dropped, 0);
    }

    #[test]
    fn empty_batch_with_fresh_prior_state() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(now - TimeDelta::seconds(1)),
        };

        let output = resolve(vec![], now, Some(&prev), &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Deterministic,
            "no new events but prior deterministic is still fresh"
        );
        assert!(!output.result.is_fallback);
        assert!(!output.result.re_promoted);
    }

    #[test]
    fn empty_batch_with_stale_prior_state() {
        let now = Utc::now();
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(
                now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 1),
            ),
        };

        let output = resolve(vec![], now, Some(&prev), &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Heuristic,
            "stale prior -> fallback"
        );
        assert!(output.result.is_fallback);
    }

    #[test]
    fn next_state_tracks_deterministic_last_seen() {
        let now = Utc::now();
        let det_time = now - TimeDelta::seconds(1);
        let det_event = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            det_time,
        );

        let output = resolve(vec![det_event], now, None, &ranks());

        assert_eq!(
            output.next_state.deterministic_last_seen,
            Some(det_time),
            "next state should track deterministic observation time"
        );
    }

    #[test]
    fn next_state_preserves_newer_deterministic_last_seen() {
        let now = Utc::now();
        let prev_time = now - TimeDelta::seconds(1);
        let older_batch_time = now - TimeDelta::seconds(2);

        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(prev_time),
        };

        let det_event = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            older_batch_time,
        );

        let output = resolve(vec![det_event], now, Some(&prev), &ranks());

        assert_eq!(
            output.next_state.deterministic_last_seen,
            Some(prev_time),
            "should keep the newer of prev and batch timestamps"
        );
    }

    // ─── Heuristic-Only Batch ────────────────────────────────────

    #[test]
    fn heuristic_only_batch_no_prior_deterministic() {
        let now = Utc::now();
        let poll_event = make_event("evt-1", Provider::Codex, SourceKind::Poller, "sess-1", now);

        let output = resolve(vec![poll_event], now, None, &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Heuristic,
            "no deterministic source ever seen -> heuristic"
        );
        assert!(output.result.is_fallback);
        assert_eq!(output.accepted_events.len(), 1);
        assert_eq!(output.accepted_events[0].source_kind, SourceKind::Poller);
    }

    #[test]
    fn source_rank_poller_alone_not_suppressed() {
        let now = Utc::now();
        let poll_event = make_event("evt-1", Provider::Claude, SourceKind::Poller, "sess-1", now);

        let output = resolve(vec![poll_event], now, None, &ranks());

        assert!(
            output
                .suppressed_events
                .iter()
                .all(|e| e.source_kind != SourceKind::Poller),
            "single poller should not be rank-suppressed"
        );
        assert_eq!(output.accepted_events.len(), 1);
    }

    // ─── Replay: Deterministic Drop -> Fallback -> Recovery ──────

    #[test]
    fn replay_deterministic_drop_fallback_and_recovery() {
        let t0 = Utc::now();

        // Phase 1: Fresh deterministic
        let det_event = make_event(
            "evt-1",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            t0,
        );
        let out1 = resolve(vec![det_event], t0, None, &ranks());
        assert_eq!(out1.result.winner_tier, EvidenceTier::Deterministic);
        assert!(!out1.result.is_fallback);
        assert!(!out1.result.re_promoted);

        // Phase 2: Time passes, deterministic goes stale, only poller arrives
        let t1 = t0 + TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 2);
        let poll_event = make_event(
            "evt-2",
            Provider::Codex,
            SourceKind::Poller,
            "sess-1",
            t1 - TimeDelta::seconds(1),
        );
        let out2 = resolve(vec![poll_event], t1, Some(&out1.next_state), &ranks());
        assert_eq!(
            out2.result.winner_tier,
            EvidenceTier::Heuristic,
            "phase 2: stale -> heuristic fallback"
        );
        assert!(out2.result.is_fallback);
        assert!(!out2.result.re_promoted);

        // Phase 3: Deterministic source recovers
        let t2 = t1 + TimeDelta::seconds(5);
        let det_event2 = make_event(
            "evt-3",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            t2 - TimeDelta::seconds(1),
        );
        let out3 = resolve(vec![det_event2], t2, Some(&out2.next_state), &ranks());
        assert_eq!(
            out3.result.winner_tier,
            EvidenceTier::Deterministic,
            "phase 3: re-promoted to deterministic"
        );
        assert!(!out3.result.is_fallback);
        assert!(
            out3.result.re_promoted,
            "phase 3: re_promoted flag should be set"
        );

        // Phase 4: Deterministic continues fresh (no re-promotion)
        let t3 = t2 + TimeDelta::seconds(2);
        let det_event3 = make_event(
            "evt-4",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            t3 - TimeDelta::seconds(1),
        );
        let out4 = resolve(vec![det_event3], t3, Some(&out3.next_state), &ranks());
        assert_eq!(out4.result.winner_tier, EvidenceTier::Deterministic);
        assert!(!out4.result.is_fallback);
        assert!(
            !out4.result.re_promoted,
            "phase 4: no re-promotion since already deterministic"
        );
    }

    #[test]
    fn replay_down_deterministic_extended_outage() {
        let t0 = Utc::now();

        // Phase 1: Fresh deterministic
        let det_event = make_event(
            "evt-1",
            Provider::Claude,
            SourceKind::ClaudeHooks,
            "sess-1",
            t0,
        );
        let out1 = resolve(vec![det_event], t0, None, &ranks());
        assert_eq!(out1.result.winner_tier, EvidenceTier::Deterministic);

        // Phase 2: Extended outage (> DOWN_THRESHOLD_SECS)
        let t1 = t0 + TimeDelta::seconds(DOWN_THRESHOLD_SECS as i64 + 5);
        let poll_event = make_event(
            "evt-2",
            Provider::Claude,
            SourceKind::Poller,
            "sess-1",
            t1 - TimeDelta::seconds(1),
        );
        let out2 = resolve(vec![poll_event], t1, Some(&out1.next_state), &ranks());
        assert_eq!(out2.result.winner_tier, EvidenceTier::Heuristic);
        assert!(out2.result.is_fallback);

        // Phase 3: Recovery
        let t2 = t1 + TimeDelta::seconds(2);
        let det_event2 = make_event(
            "evt-3",
            Provider::Claude,
            SourceKind::ClaudeHooks,
            "sess-1",
            t2,
        );
        let out3 = resolve(vec![det_event2], t2, Some(&out2.next_state), &ranks());
        assert_eq!(out3.result.winner_tier, EvidenceTier::Deterministic);
        assert!(out3.result.re_promoted);
    }

    // ─── Stale Det + Poller: Poller Must Survive ──────────────────

    #[test]
    fn stale_det_with_poller_poller_accepted() {
        // Regression: when stale deterministic and poller arrive in the same
        // batch, the poller must NOT be rank-suppressed. The winner tier is
        // heuristic (det is stale), so poller is in the winning tier.
        let now = Utc::now();
        let stale_det_time = now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 2);
        let det_event = make_event(
            "evt-det",
            Provider::Codex,
            SourceKind::CodexAppserver,
            "sess-1",
            stale_det_time,
        );
        let poll_event = make_event(
            "evt-poll",
            Provider::Codex,
            SourceKind::Poller,
            "sess-1",
            now - TimeDelta::seconds(1),
        );

        let output = resolve(vec![det_event, poll_event], now, None, &ranks());

        assert_eq!(
            output.result.winner_tier,
            EvidenceTier::Heuristic,
            "stale det -> heuristic fallback"
        );
        assert!(output.result.is_fallback);
        assert_eq!(
            output.accepted_events.len(),
            1,
            "poller should be the only accepted event"
        );
        assert_eq!(
            output.accepted_events[0].source_kind,
            SourceKind::Poller,
            "accepted event should be the poller"
        );
        // The stale deterministic event should be tier-suppressed
        assert_eq!(output.suppressed_events.len(), 1);
        assert_eq!(
            output.suppressed_events[0].source_kind,
            SourceKind::CodexAppserver,
            "stale det event should be tier-suppressed"
        );
    }

    #[test]
    fn stale_det_with_poller_prev_state() {
        // Same scenario but with prev_state tracking the stale det.
        let now = Utc::now();
        let stale_time = now - TimeDelta::seconds(FRESH_THRESHOLD_SECS as i64 + 2);
        let prev = ResolverState {
            current_tier: EvidenceTier::Deterministic,
            deterministic_last_seen: Some(stale_time),
        };
        let poll_event = make_event(
            "evt-poll",
            Provider::Codex,
            SourceKind::Poller,
            "sess-1",
            now - TimeDelta::seconds(1),
        );

        let output = resolve(vec![poll_event], now, Some(&prev), &ranks());

        assert_eq!(output.result.winner_tier, EvidenceTier::Heuristic);
        assert_eq!(output.accepted_events.len(), 1);
        assert_eq!(output.accepted_events[0].source_kind, SourceKind::Poller);
    }

    // ─── Rank Suppression Within Winner Tier ────────────────────

    #[test]
    fn rank_suppression_within_heuristic_tier() {
        // When winner is heuristic and multiple heuristic sources exist for
        // same provider, rank suppression still applies within the tier.
        // (This is a hypothetical scenario; currently all heuristic sources
        // are Poller so there's no within-tier rank suppression.)
        let now = Utc::now();
        let poll_event = make_event(
            "evt-poll",
            Provider::Claude,
            SourceKind::Poller,
            "sess-1",
            now,
        );

        let output = resolve(vec![poll_event], now, None, &ranks());

        assert_eq!(output.result.winner_tier, EvidenceTier::Heuristic);
        assert_eq!(output.accepted_events.len(), 1);
        assert!(output.suppressed_events.is_empty());
    }

    // ─── Resolver State Default ──────────────────────────────────

    #[test]
    fn resolver_state_default() {
        let state = ResolverState::default();
        assert_eq!(state.current_tier, EvidenceTier::Heuristic);
        assert!(state.deterministic_last_seen.is_none());
    }
}
