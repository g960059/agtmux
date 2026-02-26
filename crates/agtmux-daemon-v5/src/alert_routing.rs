//! Observability alert routing with warn/degraded/escalate levels,
//! diagnostics hooks, and alert ledger sink.
//!
//! Pure, testable state machine with no IO or async dependencies.
//!
//! Task ref: T-051

use serde::{Deserialize, Serialize};

// ─── Alert Severity ──────────────────────────────────────────────

/// Severity level for an alert entry.
///
/// Ordered: `Info` < `Warn` < `Degraded` < `Escalate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warn,
    Degraded,
    Escalate,
}

// ─── Alert Entry ─────────────────────────────────────────────────

/// A single alert record stored in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertEntry {
    /// Unique identifier for this alert.
    pub alert_id: String,
    /// Severity level.
    pub severity: AlertSeverity,
    /// Subsystem that produced the alert (e.g. `"latency_window"`, `"source_health"`, `"supervisor"`).
    pub source: String,
    /// Human-readable description.
    pub message: String,
    /// Creation timestamp in epoch milliseconds.
    pub created_at_ms: u64,
    /// Resolution timestamp in epoch milliseconds, if resolved.
    pub resolved_at_ms: Option<u64>,
    /// Resolve policy for this entry (inherited from router default at emit time).
    pub resolve_policy: ResolvePolicy,
}

// ─── Resolve Policy ──────────────────────────────────────────────

/// Policy governing how alerts are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvePolicy {
    /// Auto-resolve when condition clears.
    AutoResolve,
    /// Require manual acknowledgement.
    ManualAck,
}

// ─── Alert Router ────────────────────────────────────────────────

/// Routes, stores, and manages observability alerts.
///
/// Maintains an append-only ledger of [`AlertEntry`] records.
/// Supports emit, resolve, auto-resolve-by-source, severity filtering,
/// and pruning of old resolved entries.
pub struct AlertRouter {
    ledger: Vec<AlertEntry>,
    next_id: u64,
    /// Default resolve policy for new alerts.
    default_policy: ResolvePolicy,
}

impl AlertRouter {
    /// Create a new router with the default `AutoResolve` policy.
    pub fn new() -> Self {
        Self {
            ledger: Vec::new(),
            next_id: 1,
            default_policy: ResolvePolicy::AutoResolve,
        }
    }

    /// Create a new router with a specific resolve policy.
    pub fn with_policy(policy: ResolvePolicy) -> Self {
        Self {
            ledger: Vec::new(),
            next_id: 1,
            default_policy: policy,
        }
    }

    /// Emit a new alert. Returns the `alert_id`.
    pub fn emit(
        &mut self,
        severity: AlertSeverity,
        source: &str,
        message: &str,
        now_ms: u64,
    ) -> String {
        let alert_id = format!("alert-{}", self.next_id);
        self.next_id += 1;

        self.ledger.push(AlertEntry {
            alert_id: alert_id.clone(),
            severity,
            source: source.to_owned(),
            message: message.to_owned(),
            created_at_ms: now_ms,
            resolved_at_ms: None,
            resolve_policy: self.default_policy,
        });

        alert_id
    }

    /// Resolve an alert by ID (mark `resolved_at_ms`).
    ///
    /// Returns `false` if the alert is not found or already resolved.
    pub fn resolve(&mut self, alert_id: &str, now_ms: u64) -> bool {
        for entry in &mut self.ledger {
            if entry.alert_id == alert_id {
                if entry.resolved_at_ms.is_some() {
                    return false;
                }
                entry.resolved_at_ms = Some(now_ms);
                return true;
            }
        }
        false
    }

    /// Auto-resolve all unresolved alerts from a given source.
    ///
    /// Only resolves entries whose `resolve_policy` is `AutoResolve`.
    /// Entries with `ManualAck` are left unresolved — use [`resolve`] by ID instead.
    ///
    /// Returns the count of alerts resolved.
    pub fn auto_resolve_source(&mut self, source: &str, now_ms: u64) -> usize {
        let mut count = 0;
        for entry in &mut self.ledger {
            if entry.source == source
                && entry.resolved_at_ms.is_none()
                && entry.resolve_policy == ResolvePolicy::AutoResolve
            {
                entry.resolved_at_ms = Some(now_ms);
                count += 1;
            }
        }
        count
    }

    /// Get all unresolved alerts.
    pub fn unresolved(&self) -> Vec<&AlertEntry> {
        self.ledger
            .iter()
            .filter(|e| e.resolved_at_ms.is_none())
            .collect()
    }

    /// Get all unresolved alerts at or above a given severity.
    pub fn unresolved_at_severity(&self, min_severity: AlertSeverity) -> Vec<&AlertEntry> {
        self.ledger
            .iter()
            .filter(|e| e.resolved_at_ms.is_none() && e.severity >= min_severity)
            .collect()
    }

    /// Get a specific alert by ID.
    pub fn get(&self, alert_id: &str) -> Option<&AlertEntry> {
        self.ledger.iter().find(|e| e.alert_id == alert_id)
    }

    /// Total number of alerts in the ledger (both resolved and unresolved).
    pub fn ledger_size(&self) -> usize {
        self.ledger.len()
    }

    /// Prune resolved alerts older than `before_ms`.
    ///
    /// Returns the count of entries removed.
    pub fn prune_resolved(&mut self, before_ms: u64) -> usize {
        let original_len = self.ledger.len();
        self.ledger.retain(|entry| {
            match entry.resolved_at_ms {
                Some(resolved_at) => resolved_at >= before_ms,
                // Keep all unresolved entries.
                None => true,
            }
        });
        original_len - self.ledger.len()
    }
}

impl Default for AlertRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_router_no_alerts() {
        let router = AlertRouter::new();
        assert_eq!(router.ledger_size(), 0);
        assert!(router.unresolved().is_empty());
    }

    #[test]
    fn emit_creates_alert() {
        let mut router = AlertRouter::new();
        let id = router.emit(AlertSeverity::Warn, "latency_window", "high p99", 1000);
        assert!(!id.is_empty());
        assert_eq!(router.ledger_size(), 1);
        let entry = router.get(&id);
        assert!(entry.is_some());
        let entry = entry.expect("alert should exist");
        assert_eq!(entry.severity, AlertSeverity::Warn);
        assert_eq!(entry.source, "latency_window");
        assert_eq!(entry.message, "high p99");
        assert_eq!(entry.created_at_ms, 1000);
        assert!(entry.resolved_at_ms.is_none());
    }

    #[test]
    fn emit_increments_id() {
        let mut router = AlertRouter::new();
        let id1 = router.emit(AlertSeverity::Info, "src1", "msg1", 100);
        let id2 = router.emit(AlertSeverity::Info, "src2", "msg2", 200);
        let id3 = router.emit(AlertSeverity::Info, "src3", "msg3", 300);
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn resolve_marks_resolved() {
        let mut router = AlertRouter::new();
        let id = router.emit(AlertSeverity::Degraded, "source_health", "down", 1000);
        let resolved = router.resolve(&id, 2000);
        assert!(resolved);
        let entry = router.get(&id).expect("alert should exist");
        assert_eq!(entry.resolved_at_ms, Some(2000));
    }

    #[test]
    fn resolve_unknown_returns_false() {
        let mut router = AlertRouter::new();
        assert!(!router.resolve("nonexistent-id", 1000));
    }

    #[test]
    fn resolve_already_resolved_returns_false() {
        let mut router = AlertRouter::new();
        let id = router.emit(AlertSeverity::Warn, "supervisor", "restart", 1000);
        assert!(router.resolve(&id, 2000));
        assert!(!router.resolve(&id, 3000));
    }

    #[test]
    fn unresolved_filters_correctly() {
        let mut router = AlertRouter::new();
        let id1 = router.emit(AlertSeverity::Info, "a", "m1", 100);
        let _id2 = router.emit(AlertSeverity::Warn, "b", "m2", 200);
        let id3 = router.emit(AlertSeverity::Escalate, "c", "m3", 300);

        // Resolve id1 and id3, leaving id2 unresolved.
        router.resolve(&id1, 400);
        router.resolve(&id3, 500);

        let unresolved = router.unresolved();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].source, "b");
    }

    #[test]
    fn unresolved_at_severity_filters() {
        let mut router = AlertRouter::new();
        router.emit(AlertSeverity::Info, "a", "info", 100);
        router.emit(AlertSeverity::Warn, "b", "warn", 200);
        router.emit(AlertSeverity::Degraded, "c", "degraded", 300);
        router.emit(AlertSeverity::Escalate, "d", "escalate", 400);

        // Filter at Degraded: should return Degraded + Escalate.
        let filtered = router.unresolved_at_severity(AlertSeverity::Degraded);
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|e| e.severity >= AlertSeverity::Degraded)
        );

        // Filter at Warn: should return Warn + Degraded + Escalate.
        let filtered = router.unresolved_at_severity(AlertSeverity::Warn);
        assert_eq!(filtered.len(), 3);

        // Filter at Info: should return all 4.
        let filtered = router.unresolved_at_severity(AlertSeverity::Info);
        assert_eq!(filtered.len(), 4);
    }

    #[test]
    fn severity_ordering() {
        assert!(AlertSeverity::Info < AlertSeverity::Warn);
        assert!(AlertSeverity::Warn < AlertSeverity::Degraded);
        assert!(AlertSeverity::Degraded < AlertSeverity::Escalate);
    }

    #[test]
    fn auto_resolve_source() {
        let mut router = AlertRouter::new();
        router.emit(AlertSeverity::Warn, "latency_window", "slow", 100);
        router.emit(AlertSeverity::Degraded, "latency_window", "very slow", 200);
        router.emit(AlertSeverity::Info, "other_source", "ok", 300);

        let count = router.auto_resolve_source("latency_window", 500);
        assert_eq!(count, 2);
        assert_eq!(router.unresolved().len(), 1);
        assert_eq!(router.unresolved()[0].source, "other_source");
    }

    #[test]
    fn auto_resolve_different_source_untouched() {
        let mut router = AlertRouter::new();
        router.emit(AlertSeverity::Warn, "source_a", "msg", 100);
        router.emit(AlertSeverity::Warn, "source_b", "msg", 200);

        let count = router.auto_resolve_source("source_a", 300);
        assert_eq!(count, 1);

        // source_b should still be unresolved.
        let unresolved = router.unresolved();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].source, "source_b");
    }

    #[test]
    fn prune_resolved_removes_old() {
        let mut router = AlertRouter::new();
        let id1 = router.emit(AlertSeverity::Info, "a", "old", 100);
        let id2 = router.emit(AlertSeverity::Info, "b", "recent", 500);
        router.resolve(&id1, 200);
        router.resolve(&id2, 600);

        // Prune resolved before 500: should remove id1 (resolved at 200) but keep id2 (resolved at 600).
        let removed = router.prune_resolved(500);
        assert_eq!(removed, 1);
        assert_eq!(router.ledger_size(), 1);
        assert!(router.get(&id1).is_none());
        assert!(router.get(&id2).is_some());
    }

    #[test]
    fn prune_keeps_unresolved() {
        let mut router = AlertRouter::new();
        router.emit(AlertSeverity::Warn, "a", "still active", 100);
        let id2 = router.emit(AlertSeverity::Info, "b", "old resolved", 50);
        router.resolve(&id2, 60);

        // Prune before 1000: only the resolved entry (resolved at 60) is eligible.
        let removed = router.prune_resolved(1000);
        assert_eq!(removed, 1);
        // The unresolved alert must remain.
        assert_eq!(router.ledger_size(), 1);
        assert_eq!(router.unresolved().len(), 1);
    }

    #[test]
    fn get_by_id() {
        let mut router = AlertRouter::new();
        let id = router.emit(AlertSeverity::Escalate, "supervisor", "critical", 999);
        let entry = router.get(&id).expect("should find alert by id");
        assert_eq!(entry.alert_id, id);
        assert_eq!(entry.severity, AlertSeverity::Escalate);
        assert_eq!(entry.source, "supervisor");
        assert_eq!(entry.message, "critical");
        assert_eq!(entry.created_at_ms, 999);
    }

    #[test]
    fn manual_ack_policy_blocks_auto_resolve() {
        let mut router = AlertRouter::with_policy(ResolvePolicy::ManualAck);
        let id = router.emit(AlertSeverity::Warn, "latency_window", "slow", 100);

        // auto_resolve_source should NOT resolve ManualAck entries
        let count = router.auto_resolve_source("latency_window", 200);
        assert_eq!(count, 0);
        assert_eq!(router.unresolved().len(), 1);

        // Explicit resolve by ID should still work
        assert!(router.resolve(&id, 300));
        assert!(router.unresolved().is_empty());
    }

    #[test]
    fn ledger_size_tracks_all() {
        let mut router = AlertRouter::new();
        assert_eq!(router.ledger_size(), 0);

        let id1 = router.emit(AlertSeverity::Info, "a", "m1", 100);
        assert_eq!(router.ledger_size(), 1);

        router.emit(AlertSeverity::Warn, "b", "m2", 200);
        assert_eq!(router.ledger_size(), 2);

        // Resolving does not change ledger size.
        router.resolve(&id1, 300);
        assert_eq!(router.ledger_size(), 2);
    }
}
