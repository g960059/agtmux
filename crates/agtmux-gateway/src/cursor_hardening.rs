//! Cursor contract hardening: two-watermark tracking, ack progression,
//! safe rewind, and invalid-cursor recovery.
//!
//! Task ref: T-041

use std::fmt;

// ─── Errors ──────────────────────────────────────────────────────────

/// Errors arising from cursor contract violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    /// Attempted to move cursor backward (monotonic violation).
    NonMonotonic { current: u64, attempted: u64 },
    /// Attempted to commit beyond fetched position.
    CommitAheadOfFetched { fetched: u64, attempted: u64 },
    /// Rewind too far back in events.
    RewindTooFar { max_events: u64, requested: u64 },
    /// Rewind too far back in time.
    RewindTooOld { max_secs: u64, age_secs: u64 },
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonMonotonic { current, attempted } => {
                write!(
                    f,
                    "non-monotonic cursor: current={current}, attempted={attempted}"
                )
            }
            Self::CommitAheadOfFetched { fetched, attempted } => {
                write!(
                    f,
                    "commit ahead of fetched: fetched={fetched}, attempted={attempted}"
                )
            }
            Self::RewindTooFar {
                max_events,
                requested,
            } => {
                write!(
                    f,
                    "rewind too far: max_events={max_events}, requested={requested}"
                )
            }
            Self::RewindTooOld { max_secs, age_secs } => {
                write!(
                    f,
                    "rewind too old: max_secs={max_secs}, age_secs={age_secs}"
                )
            }
        }
    }
}

impl std::error::Error for CursorError {}

// ─── Two-Watermark Cursor Tracking ──────────────────────────────────

/// Two-watermark cursor state: tracks both how far we have *fetched*
/// (sent to consumer) and how far the consumer has *committed* (acked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorWatermarks {
    /// Cursor position returned to the consumer (daemon) — "how far we've sent".
    pub fetched: u64,
    /// Cursor position acknowledged (committed) by the consumer — "how far is confirmed consumed".
    pub committed: u64,
    /// Maximum rewind distance (events).
    pub max_rewind_events: u64,
    /// Maximum rewind duration (seconds).
    pub max_rewind_secs: u64,
}

impl CursorWatermarks {
    /// Create with defaults (both watermarks at 0, max rewind 10 000 events / 600 s).
    pub fn new() -> Self {
        Self {
            fetched: 0,
            committed: 0,
            max_rewind_events: 10_000,
            max_rewind_secs: 600,
        }
    }

    /// Record that events up to `position` have been fetched (sent to consumer).
    /// Returns `Err` if position goes backward (monotonic constraint).
    pub fn advance_fetched(&mut self, position: u64) -> Result<(), CursorError> {
        if position < self.fetched {
            return Err(CursorError::NonMonotonic {
                current: self.fetched,
                attempted: position,
            });
        }
        self.fetched = position;
        Ok(())
    }

    /// Consumer commits (acknowledges) up to `position`.
    /// Must not exceed `fetched` (can't ack what hasn't been fetched).
    /// Returns `Err` if position > fetched or goes backward.
    pub fn commit(&mut self, position: u64) -> Result<(), CursorError> {
        if position < self.committed {
            return Err(CursorError::NonMonotonic {
                current: self.committed,
                attempted: position,
            });
        }
        if position > self.fetched {
            return Err(CursorError::CommitAheadOfFetched {
                fetched: self.fetched,
                attempted: position,
            });
        }
        self.committed = position;
        Ok(())
    }

    /// Check how far behind committed is from fetched.
    pub fn uncommitted_gap(&self) -> u64 {
        self.fetched.saturating_sub(self.committed)
    }

    /// Is the committed cursor current with fetched?
    pub fn is_caught_up(&self) -> bool {
        self.committed == self.fetched
    }

    /// Request a rewind to `target_position`.
    ///
    /// Validates against `max_rewind_events` and `max_rewind_secs` constraints.
    ///
    /// * `target_position` — the position to rewind to (must be < current fetched).
    /// * `now_secs` — monotonic timestamp for the current moment.
    /// * `earliest_valid_ts` — the timestamp (in monotonic seconds) of the
    ///   event at `target_position`. The rewind age is `now_secs - earliest_valid_ts`.
    pub fn safe_rewind(
        &mut self,
        target_position: u64,
        now_secs: u64,
        earliest_valid_ts: u64,
    ) -> Result<RewindResult, CursorError> {
        let events_back = self.fetched.saturating_sub(target_position);
        if events_back > self.max_rewind_events {
            return Err(CursorError::RewindTooFar {
                max_events: self.max_rewind_events,
                requested: events_back,
            });
        }

        let age_secs = now_secs.saturating_sub(earliest_valid_ts);
        if age_secs > self.max_rewind_secs {
            return Err(CursorError::RewindTooOld {
                max_secs: self.max_rewind_secs,
                age_secs,
            });
        }

        let previous_fetched = self.fetched;
        self.fetched = target_position;
        // Also reset committed if it was ahead of the new fetched position
        if self.committed > target_position {
            self.committed = target_position;
        }

        Ok(RewindResult {
            previous_fetched,
            new_fetched: target_position,
            events_rewound: events_back,
        })
    }
}

impl Default for CursorWatermarks {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Rewind Result ──────────────────────────────────────────────────

/// Outcome of a successful rewind operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindResult {
    /// Fetched position before the rewind.
    pub previous_fetched: u64,
    /// Fetched position after the rewind.
    pub new_fetched: u64,
    /// Number of events rewound.
    pub events_rewound: u64,
}

// ─── Invalid Cursor Detection & Recovery ────────────────────────────

/// Action to take when an invalid cursor is detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRecoveryAction {
    /// Try from a different position (e.g., last committed).
    RetryFromCommitted,
    /// Too many invalid attempts — full resync from beginning.
    FullResync,
}

/// Tracks consecutive invalid cursor attempts and decides recovery strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCursorTracker {
    /// Consecutive invalid cursor attempts.
    streak: u32,
    /// Threshold to trigger full resync.
    resync_threshold: u32,
}

impl InvalidCursorTracker {
    /// Create with the default threshold of 3.
    pub fn new() -> Self {
        Self {
            streak: 0,
            resync_threshold: 3,
        }
    }

    /// Create with a custom threshold.
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            streak: 0,
            resync_threshold: threshold,
        }
    }

    /// Record an invalid cursor attempt.
    /// Returns `FullResync` if streak >= threshold, else `RetryFromCommitted`.
    pub fn record_invalid(&mut self) -> CursorRecoveryAction {
        self.streak = self.streak.saturating_add(1);
        if self.streak >= self.resync_threshold {
            CursorRecoveryAction::FullResync
        } else {
            CursorRecoveryAction::RetryFromCommitted
        }
    }

    /// Reset streak on successful cursor operation.
    pub fn record_valid(&mut self) {
        self.streak = 0;
    }

    /// Current streak count.
    pub fn streak(&self) -> u32 {
        self.streak
    }
}

impl Default for InvalidCursorTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Watermark tests ─────────────────────────────────────────────

    #[test]
    fn new_watermarks_start_at_zero() {
        let wm = CursorWatermarks::new();
        assert_eq!(wm.fetched, 0);
        assert_eq!(wm.committed, 0);
        assert_eq!(wm.max_rewind_events, 10_000);
        assert_eq!(wm.max_rewind_secs, 600);
    }

    #[test]
    fn advance_fetched_monotonic() {
        let mut wm = CursorWatermarks::new();

        // Forward works
        assert!(wm.advance_fetched(10).is_ok());
        assert_eq!(wm.fetched, 10);

        // Same value works (not strictly backward)
        assert!(wm.advance_fetched(10).is_ok());
        assert_eq!(wm.fetched, 10);

        // Forward again
        assert!(wm.advance_fetched(20).is_ok());
        assert_eq!(wm.fetched, 20);

        // Backward fails
        let err = wm.advance_fetched(15).expect_err("should fail");
        assert_eq!(
            err,
            CursorError::NonMonotonic {
                current: 20,
                attempted: 15,
            }
        );
        // Position unchanged after error
        assert_eq!(wm.fetched, 20);
    }

    #[test]
    fn commit_within_fetched() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(100).expect("advance");

        // Commit up to fetched succeeds
        assert!(wm.commit(50).is_ok());
        assert_eq!(wm.committed, 50);

        assert!(wm.commit(100).is_ok());
        assert_eq!(wm.committed, 100);
    }

    #[test]
    fn commit_beyond_fetched_fails() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(50).expect("advance");

        let err = wm.commit(51).expect_err("should fail");
        assert_eq!(
            err,
            CursorError::CommitAheadOfFetched {
                fetched: 50,
                attempted: 51,
            }
        );
        assert_eq!(wm.committed, 0);
    }

    #[test]
    fn commit_backward_fails() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(100).expect("advance");
        wm.commit(50).expect("commit");

        let err = wm.commit(30).expect_err("should fail");
        assert_eq!(
            err,
            CursorError::NonMonotonic {
                current: 50,
                attempted: 30,
            }
        );
        assert_eq!(wm.committed, 50);
    }

    #[test]
    fn uncommitted_gap_calculation() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(100).expect("advance");
        wm.commit(60).expect("commit");

        assert_eq!(wm.uncommitted_gap(), 40);
    }

    #[test]
    fn is_caught_up_when_equal() {
        let mut wm = CursorWatermarks::new();
        assert!(wm.is_caught_up()); // both 0

        wm.advance_fetched(50).expect("advance");
        wm.commit(50).expect("commit");
        assert!(wm.is_caught_up());
    }

    #[test]
    fn is_caught_up_false_when_behind() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(50).expect("advance");
        wm.commit(30).expect("commit");

        assert!(!wm.is_caught_up());
    }

    // ── Rewind tests ────────────────────────────────────────────────

    #[test]
    fn safe_rewind_within_limits() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(500).expect("advance");
        wm.commit(400).expect("commit");

        // Rewind 100 events, age 60 s — within both limits
        let result = wm.safe_rewind(400, 1000, 940).expect("rewind");
        assert_eq!(result.previous_fetched, 500);
        assert_eq!(result.new_fetched, 400);
        assert_eq!(result.events_rewound, 100);
        assert_eq!(wm.fetched, 400);
    }

    #[test]
    fn safe_rewind_too_many_events() {
        let mut wm = CursorWatermarks {
            fetched: 20_000,
            committed: 10_000,
            max_rewind_events: 10_000,
            max_rewind_secs: 600,
        };

        // Rewind 10_001 events — exceeds limit
        let err = wm.safe_rewind(9_999, 1000, 990).expect_err("should fail");
        assert_eq!(
            err,
            CursorError::RewindTooFar {
                max_events: 10_000,
                requested: 10_001,
            }
        );
        // Position unchanged
        assert_eq!(wm.fetched, 20_000);
    }

    #[test]
    fn safe_rewind_too_old() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(500).expect("advance");

        // Rewind age = 1000 - 300 = 700 s, exceeds 600 s limit
        let err = wm.safe_rewind(400, 1000, 300).expect_err("should fail");
        assert_eq!(
            err,
            CursorError::RewindTooOld {
                max_secs: 600,
                age_secs: 700,
            }
        );
        assert_eq!(wm.fetched, 500);
    }

    #[test]
    fn rewind_resets_fetched() {
        let mut wm = CursorWatermarks::new();
        wm.advance_fetched(1000).expect("advance");
        wm.commit(800).expect("commit");

        wm.safe_rewind(500, 2000, 1900).expect("rewind");
        assert_eq!(wm.fetched, 500);
        // Committed is also clamped since it was > new fetched
        assert_eq!(wm.committed, 500);
    }

    // ── Invalid cursor tests ────────────────────────────────────────

    #[test]
    fn initial_streak_is_zero() {
        let tracker = InvalidCursorTracker::new();
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn record_invalid_increments_streak() {
        let mut tracker = InvalidCursorTracker::new();

        let action = tracker.record_invalid();
        assert_eq!(action, CursorRecoveryAction::RetryFromCommitted);
        assert_eq!(tracker.streak(), 1);

        let action = tracker.record_invalid();
        assert_eq!(action, CursorRecoveryAction::RetryFromCommitted);
        assert_eq!(tracker.streak(), 2);
    }

    #[test]
    fn streak_reaches_threshold_triggers_resync() {
        let mut tracker = InvalidCursorTracker::new(); // threshold = 3

        tracker.record_invalid(); // 1 → RetryFromCommitted
        tracker.record_invalid(); // 2 → RetryFromCommitted

        let action = tracker.record_invalid(); // 3 → FullResync
        assert_eq!(action, CursorRecoveryAction::FullResync);
        assert_eq!(tracker.streak(), 3);
    }

    #[test]
    fn record_valid_resets_streak() {
        let mut tracker = InvalidCursorTracker::new();

        tracker.record_invalid();
        tracker.record_invalid();
        assert_eq!(tracker.streak(), 2);

        tracker.record_valid();
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn custom_threshold() {
        let mut tracker = InvalidCursorTracker::with_threshold(5);

        for _ in 0..4 {
            let action = tracker.record_invalid();
            assert_eq!(action, CursorRecoveryAction::RetryFromCommitted);
        }

        let action = tracker.record_invalid(); // 5th → FullResync
        assert_eq!(action, CursorRecoveryAction::FullResync);
        assert_eq!(tracker.streak(), 5);
    }

    // ── Integration test ────────────────────────────────────────────

    #[test]
    fn full_ack_cycle() {
        let mut wm = CursorWatermarks::new();

        // Phase 1: advance fetched, then commit
        wm.advance_fetched(100).expect("advance 1");
        assert_eq!(wm.uncommitted_gap(), 100);
        assert!(!wm.is_caught_up());

        wm.commit(100).expect("commit 1");
        assert_eq!(wm.uncommitted_gap(), 0);
        assert!(wm.is_caught_up());

        // Phase 2: advance more, partial commit, then full commit
        wm.advance_fetched(250).expect("advance 2");
        assert_eq!(wm.uncommitted_gap(), 150);

        wm.commit(200).expect("partial commit");
        assert_eq!(wm.uncommitted_gap(), 50);
        assert!(!wm.is_caught_up());

        wm.commit(250).expect("full commit");
        assert_eq!(wm.uncommitted_gap(), 0);
        assert!(wm.is_caught_up());

        // Final state
        assert_eq!(wm.fetched, 250);
        assert_eq!(wm.committed, 250);
    }
}
