//! Snapshot/restore foundation: periodic/shutdown snapshot metadata
//! and restore dry-run checker.
//!
//! Pure, testable state machines with no IO or async dependencies.
//! The actual serialization/persistence layer is handled externally;
//! this module manages metadata, policy, and restore validation.
//!
//! Task ref: T-049

use serde::{Deserialize, Serialize};

// ─── Snapshot Trigger ───────────────────────────────────────────

/// Trigger reason for a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SnapshotTrigger {
    /// Periodic checkpoint.
    Periodic,
    /// Clean shutdown.
    Shutdown,
    /// Manual operator request.
    Manual,
}

// ─── Snapshot Metadata ──────────────────────────────────────────

/// Metadata describing a single snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Unique snapshot identifier.
    pub snapshot_id: String,
    /// Timestamp when snapshot was created (epoch ms).
    pub created_at_ms: u64,
    /// Projection version at snapshot time.
    pub projection_version: u64,
    /// Number of sessions in the snapshot.
    pub session_count: usize,
    /// Number of panes in the snapshot.
    pub pane_count: usize,
    /// Trigger reason for the snapshot.
    pub trigger: SnapshotTrigger,
    /// Size in bytes of the serialized state (for observability).
    pub size_bytes: u64,
}

// ─── Snapshot Policy ────────────────────────────────────────────

/// Configuration governing snapshot frequency and retention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPolicy {
    /// Interval between periodic snapshots in milliseconds (default 300_000 = 5min).
    pub interval_ms: u64,
    /// Maximum age of a snapshot in milliseconds before it's considered expired (default 600_000 = 10min).
    pub max_age_ms: u64,
    /// Maximum number of snapshots to retain (default 3).
    pub max_retained: usize,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            interval_ms: 300_000,
            max_age_ms: 600_000,
            max_retained: 3,
        }
    }
}

// ─── Snapshot Manager ───────────────────────────────────────────

/// Manages snapshot metadata, scheduling, and retention.
///
/// Pure, deterministic state machine. All time values are passed in
/// as parameters (no system clock access).
pub struct SnapshotManager {
    policy: SnapshotPolicy,
    snapshots: Vec<SnapshotMetadata>,
    last_snapshot_ms: Option<u64>,
    next_snapshot_id: u64,
}

impl SnapshotManager {
    /// Create a new manager with the given policy.
    pub fn new(policy: SnapshotPolicy) -> Self {
        Self {
            policy,
            snapshots: Vec::new(),
            last_snapshot_ms: None,
            next_snapshot_id: 1,
        }
    }

    /// Check if a periodic snapshot is due.
    ///
    /// Returns `true` when no snapshot has been taken yet, or when
    /// `now_ms` is at least `policy.interval_ms` past the last snapshot.
    pub fn is_snapshot_due(&self, now_ms: u64) -> bool {
        match self.last_snapshot_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= self.policy.interval_ms,
        }
    }

    /// Record a snapshot with the given metrics. Returns the metadata.
    ///
    /// Assigns a monotonically increasing snapshot ID and updates
    /// the last-snapshot timestamp.
    pub fn record_snapshot(
        &mut self,
        trigger: SnapshotTrigger,
        now_ms: u64,
        projection_version: u64,
        session_count: usize,
        pane_count: usize,
        size_bytes: u64,
    ) -> SnapshotMetadata {
        let id = self.next_snapshot_id;
        self.next_snapshot_id += 1;

        let metadata = SnapshotMetadata {
            snapshot_id: format!("snap-{id}"),
            created_at_ms: now_ms,
            projection_version,
            session_count,
            pane_count,
            trigger,
            size_bytes,
        };

        self.snapshots.push(metadata.clone());
        self.last_snapshot_ms = Some(now_ms);

        metadata
    }

    /// Get the latest snapshot.
    pub fn latest(&self) -> Option<&SnapshotMetadata> {
        self.snapshots.last()
    }

    /// List all retained snapshots.
    pub fn list(&self) -> &[SnapshotMetadata] {
        &self.snapshots
    }

    /// Prune snapshots beyond the retention limit (keeps newest).
    ///
    /// Returns the number of snapshots removed.
    pub fn prune(&mut self) -> usize {
        let len = self.snapshots.len();
        if len <= self.policy.max_retained {
            return 0;
        }
        let to_remove = len - self.policy.max_retained;
        self.snapshots.drain(..to_remove);
        to_remove
    }

    /// Policy accessor.
    pub fn policy(&self) -> &SnapshotPolicy {
        &self.policy
    }
}

// ─── Restore Dry-Run Checker ────────────────────────────────────

/// Verdict from a restore dry-run check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreVerdict {
    /// Snapshot is valid and can be restored.
    Ok {
        /// Age of the snapshot in milliseconds.
        age_ms: u64,
    },
    /// Snapshot is too old.
    TooOld {
        /// Actual age of the snapshot in milliseconds.
        age_ms: u64,
        /// Maximum allowed age in milliseconds.
        max_age_ms: u64,
    },
    /// Snapshot version is ahead of current (data corruption risk).
    VersionAhead {
        /// Projection version in the snapshot.
        snapshot_version: u64,
        /// Current projection version.
        current_version: u64,
    },
}

/// Dry-run checker for snapshot restore safety.
///
/// Validates that a snapshot is safe to restore without actually
/// performing the restore. Checks age and version compatibility.
pub struct RestoreDryRun {
    /// The snapshot to validate.
    pub snapshot: SnapshotMetadata,
    /// Current wall-clock time in epoch milliseconds.
    pub current_time_ms: u64,
}

impl RestoreDryRun {
    /// Create a new dry-run checker.
    pub fn new(snapshot: SnapshotMetadata, current_time_ms: u64) -> Self {
        Self {
            snapshot,
            current_time_ms,
        }
    }

    /// Check if the snapshot can be safely restored.
    ///
    /// Checks are evaluated in priority order:
    /// 1. Version ahead → `VersionAhead` (corruption risk).
    /// 2. Age exceeds `max_age_ms` → `TooOld`.
    /// 3. Otherwise → `Ok`.
    pub fn check(&self, current_version: u64, max_age_ms: u64) -> RestoreVerdict {
        // 1. Version-ahead check (highest priority — corruption risk).
        if self.snapshot.projection_version > current_version {
            return RestoreVerdict::VersionAhead {
                snapshot_version: self.snapshot.projection_version,
                current_version,
            };
        }

        // 2. Age check.
        let age_ms = self
            .current_time_ms
            .saturating_sub(self.snapshot.created_at_ms);
        if age_ms > max_age_ms {
            return RestoreVerdict::TooOld { age_ms, max_age_ms };
        }

        // 3. All clear.
        RestoreVerdict::Ok { age_ms }
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Policy defaults ────────────────────────────────────────

    #[test]
    fn default_policy_values() {
        let p = SnapshotPolicy::default();
        assert_eq!(p.interval_ms, 300_000); // 5 min
        assert_eq!(p.max_age_ms, 600_000); // 10 min
        assert_eq!(p.max_retained, 3);
    }

    // ── Snapshot scheduling ────────────────────────────────────

    #[test]
    fn snapshot_due_after_interval() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());

        // First snapshot is always due (no previous snapshot).
        assert!(mgr.is_snapshot_due(0));

        // Take a snapshot at t=0.
        mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 2, 100);

        // Not due immediately after.
        assert!(!mgr.is_snapshot_due(1_000));

        // Due after the interval elapses.
        assert!(mgr.is_snapshot_due(300_000));
    }

    #[test]
    fn snapshot_not_due_before_interval() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 2, 100);

        // Just before the interval.
        assert!(!mgr.is_snapshot_due(299_999));
    }

    // ── Record snapshot ────────────────────────────────────────

    #[test]
    fn record_snapshot_creates_metadata() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        let meta = mgr.record_snapshot(SnapshotTrigger::Periodic, 1000, 42, 3, 7, 2048);

        assert_eq!(meta.snapshot_id, "snap-1");
        assert_eq!(meta.created_at_ms, 1000);
        assert_eq!(meta.projection_version, 42);
        assert_eq!(meta.session_count, 3);
        assert_eq!(meta.pane_count, 7);
        assert_eq!(meta.trigger, SnapshotTrigger::Periodic);
        assert_eq!(meta.size_bytes, 2048);
    }

    #[test]
    fn record_snapshot_increments_id() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        let m1 = mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 1, 100);
        let m2 = mgr.record_snapshot(SnapshotTrigger::Periodic, 1000, 2, 1, 1, 100);
        let m3 = mgr.record_snapshot(SnapshotTrigger::Periodic, 2000, 3, 1, 1, 100);

        assert_eq!(m1.snapshot_id, "snap-1");
        assert_eq!(m2.snapshot_id, "snap-2");
        assert_eq!(m3.snapshot_id, "snap-3");
    }

    // ── Latest ─────────────────────────────────────────────────

    #[test]
    fn latest_returns_newest() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 1, 100);
        mgr.record_snapshot(SnapshotTrigger::Periodic, 1000, 2, 1, 1, 200);
        let m3 = mgr.record_snapshot(SnapshotTrigger::Periodic, 2000, 3, 2, 4, 300);

        let latest = mgr.latest().expect("should have a snapshot");
        assert_eq!(latest, &m3);
    }

    #[test]
    fn empty_manager_no_latest() {
        let mgr = SnapshotManager::new(SnapshotPolicy::default());
        assert!(mgr.latest().is_none());
    }

    // ── List ───────────────────────────────────────────────────

    #[test]
    fn list_returns_all() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 1, 100);
        mgr.record_snapshot(SnapshotTrigger::Shutdown, 1000, 2, 1, 1, 200);
        mgr.record_snapshot(SnapshotTrigger::Manual, 2000, 3, 2, 4, 300);

        let list = mgr.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].snapshot_id, "snap-1");
        assert_eq!(list[1].snapshot_id, "snap-2");
        assert_eq!(list[2].snapshot_id, "snap-3");
    }

    // ── Prune ──────────────────────────────────────────────────

    #[test]
    fn prune_keeps_max_retained() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default()); // max_retained=3

        for i in 0..5 {
            mgr.record_snapshot(SnapshotTrigger::Periodic, i * 1000, i + 1, 1, 1, 100);
        }
        assert_eq!(mgr.list().len(), 5);

        let removed = mgr.prune();
        assert_eq!(removed, 2);
        assert_eq!(mgr.list().len(), 3);

        // Kept the newest 3: snap-3, snap-4, snap-5.
        assert_eq!(mgr.list()[0].snapshot_id, "snap-3");
        assert_eq!(mgr.list()[1].snapshot_id, "snap-4");
        assert_eq!(mgr.list()[2].snapshot_id, "snap-5");
    }

    #[test]
    fn prune_noop_when_under_limit() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default()); // max_retained=3
        mgr.record_snapshot(SnapshotTrigger::Periodic, 0, 1, 1, 1, 100);
        mgr.record_snapshot(SnapshotTrigger::Periodic, 1000, 2, 1, 1, 200);

        let removed = mgr.prune();
        assert_eq!(removed, 0);
        assert_eq!(mgr.list().len(), 2);
    }

    // ── Trigger types ──────────────────────────────────────────

    #[test]
    fn shutdown_trigger_records() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        let meta = mgr.record_snapshot(SnapshotTrigger::Shutdown, 0, 1, 1, 1, 100);
        assert_eq!(meta.trigger, SnapshotTrigger::Shutdown);
    }

    #[test]
    fn manual_trigger_records() {
        let mut mgr = SnapshotManager::new(SnapshotPolicy::default());
        let meta = mgr.record_snapshot(SnapshotTrigger::Manual, 0, 1, 1, 1, 100);
        assert_eq!(meta.trigger, SnapshotTrigger::Manual);
    }

    // ── Restore dry-run ────────────────────────────────────────

    #[test]
    fn restore_ok_within_age() {
        let snapshot = SnapshotMetadata {
            snapshot_id: "snap-1".to_owned(),
            created_at_ms: 1_000_000,
            projection_version: 10,
            session_count: 2,
            pane_count: 5,
            trigger: SnapshotTrigger::Periodic,
            size_bytes: 4096,
        };

        let checker = RestoreDryRun::new(snapshot, 1_300_000); // 300s old
        let verdict = checker.check(10, 600_000);
        assert_eq!(verdict, RestoreVerdict::Ok { age_ms: 300_000 });
    }

    #[test]
    fn restore_too_old() {
        let snapshot = SnapshotMetadata {
            snapshot_id: "snap-1".to_owned(),
            created_at_ms: 1_000_000,
            projection_version: 10,
            session_count: 2,
            pane_count: 5,
            trigger: SnapshotTrigger::Periodic,
            size_bytes: 4096,
        };

        let checker = RestoreDryRun::new(snapshot, 2_000_000); // 1000s old
        let verdict = checker.check(10, 600_000);
        assert_eq!(
            verdict,
            RestoreVerdict::TooOld {
                age_ms: 1_000_000,
                max_age_ms: 600_000,
            }
        );
    }

    #[test]
    fn restore_version_ahead() {
        let snapshot = SnapshotMetadata {
            snapshot_id: "snap-1".to_owned(),
            created_at_ms: 1_000_000,
            projection_version: 50,
            session_count: 2,
            pane_count: 5,
            trigger: SnapshotTrigger::Periodic,
            size_bytes: 4096,
        };

        let checker = RestoreDryRun::new(snapshot, 1_100_000); // recent, but version ahead
        let verdict = checker.check(30, 600_000);
        assert_eq!(
            verdict,
            RestoreVerdict::VersionAhead {
                snapshot_version: 50,
                current_version: 30,
            }
        );
    }
}
