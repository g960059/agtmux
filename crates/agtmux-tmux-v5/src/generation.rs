//! Pane generation tracker: detects pane reuse by tracking pane_id lifetimes.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

/// Tracks pane generations to detect pane reuse.
///
/// When a pane_id disappears and reappears, the generation counter
/// increments to signal that it's a new logical pane.
#[derive(Debug, Clone, Default)]
pub struct PaneGenerationTracker {
    map: HashMap<String, (u64, DateTime<Utc>)>,
}

impl PaneGenerationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update tracker with current set of pane IDs.
    ///
    /// New pane IDs get generation 0 and `now` as birth_ts.
    /// Existing pane IDs keep their current generation.
    pub fn update(&mut self, current_pane_ids: &[&str], now: DateTime<Utc>) {
        for &pane_id in current_pane_ids {
            self.map.entry(pane_id.to_string()).or_insert((0, now));
        }
    }

    /// Get generation and birth_ts for a pane.
    pub fn get(&self, pane_id: &str) -> Option<(u64, DateTime<Utc>)> {
        self.map.get(pane_id).copied()
    }

    /// Bump generation for a pane (e.g., when we detect it has been reused).
    pub fn bump(&mut self, pane_id: &str, now: DateTime<Utc>) {
        let entry = self.map.entry(pane_id.to_string()).or_insert((0, now));
        entry.0 += 1;
        entry.1 = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid")
            .with_timezone(&Utc)
    }

    #[test]
    fn new_pane_gets_generation_zero() {
        let mut tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        tracker.update(&["%0", "%1"], now);

        let (generation, birth) = tracker.get("%0").expect("tracked");
        assert_eq!(generation, 0);
        assert_eq!(birth, now);
    }

    #[test]
    fn existing_pane_keeps_generation() {
        let mut tracker = PaneGenerationTracker::new();
        let t1 = ts("2026-02-25T12:00:00Z");
        let t2 = ts("2026-02-25T12:01:00Z");

        tracker.update(&["%0"], t1);
        tracker.update(&["%0"], t2);

        let (generation, birth) = tracker.get("%0").expect("tracked");
        assert_eq!(generation, 0);
        assert_eq!(birth, t1);
    }

    #[test]
    fn bump_increments_generation() {
        let mut tracker = PaneGenerationTracker::new();
        let t1 = ts("2026-02-25T12:00:00Z");
        let t2 = ts("2026-02-25T12:01:00Z");

        tracker.update(&["%0"], t1);
        tracker.bump("%0", t2);

        let (generation, birth) = tracker.get("%0").expect("tracked");
        assert_eq!(generation, 1);
        assert_eq!(birth, t2);
    }

    #[test]
    fn unknown_pane_returns_none() {
        let tracker = PaneGenerationTracker::new();
        assert!(tracker.get("%99").is_none());
    }

    #[test]
    fn multiple_bumps() {
        let mut tracker = PaneGenerationTracker::new();
        let t1 = ts("2026-02-25T12:00:00Z");
        let t2 = ts("2026-02-25T12:01:00Z");
        let t3 = ts("2026-02-25T12:02:00Z");

        tracker.update(&["%0"], t1);
        tracker.bump("%0", t2);
        tracker.bump("%0", t3);

        let (generation, _) = tracker.get("%0").expect("tracked");
        assert_eq!(generation, 2);
    }
}
