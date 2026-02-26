//! UDS trust admission guard: validates peer UID, source registry,
//! and runtime nonce for Unix Domain Socket connections.
//!
//! Task ref: T-047

use std::collections::HashSet;
use std::fmt;

// ─── Types ──────────────────────────────────────────────────────────

/// Result of an admission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionResult {
    /// Connection admitted.
    Admitted,
    /// Connection rejected with a reason.
    Rejected(RejectionReason),
}

/// Reason a connection was rejected by the admission guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Peer UID does not match expected.
    PeerUidMismatch { expected: u32, actual: u32 },
    /// Source is not in the registry.
    SourceNotRegistered { source_id: String },
    /// Runtime nonce does not match.
    NonceMismatch { expected: String, actual: String },
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PeerUidMismatch { expected, actual } => {
                write!(f, "peer UID mismatch: expected={expected}, actual={actual}")
            }
            Self::SourceNotRegistered { source_id } => {
                write!(f, "source not registered: {source_id}")
            }
            Self::NonceMismatch { expected, actual } => {
                write!(f, "nonce mismatch: expected={expected}, actual={actual}")
            }
        }
    }
}

// ─── TrustGuard ─────────────────────────────────────────────────────

/// Admission guard for UDS connections.
///
/// Validates three checks in order:
/// 1. Peer UID must match the daemon's UID (same user).
/// 2. Source must be registered in the source registry.
/// 3. Runtime nonce must match (prevents stale/replayed connections).
#[derive(Debug, Clone)]
pub struct TrustGuard {
    /// Expected peer UID (typically the daemon's own UID).
    expected_uid: u32,
    /// Set of registered source IDs.
    registered_sources: HashSet<String>,
    /// Current runtime nonce (rotated on daemon restart).
    runtime_nonce: String,
}

impl TrustGuard {
    /// Create a new admission guard with the given expected UID and runtime nonce.
    pub fn new(expected_uid: u32, runtime_nonce: String) -> Self {
        Self {
            expected_uid,
            registered_sources: HashSet::new(),
            runtime_nonce,
        }
    }

    /// Register a source as trusted.
    pub fn register_source(&mut self, source_id: &str) {
        self.registered_sources.insert(source_id.to_owned());
    }

    /// Unregister (revoke) a source. Returns `true` if the source was present.
    pub fn unregister_source(&mut self, source_id: &str) -> bool {
        self.registered_sources.remove(source_id)
    }

    /// Check if a source is registered.
    pub fn is_registered(&self, source_id: &str) -> bool {
        self.registered_sources.contains(source_id)
    }

    /// Number of registered sources.
    pub fn registered_count(&self) -> usize {
        self.registered_sources.len()
    }

    /// Rotate the runtime nonce (e.g., on daemon restart).
    pub fn rotate_nonce(&mut self, new_nonce: String) {
        self.runtime_nonce = new_nonce;
    }

    /// Get the runtime nonce.
    pub fn nonce(&self) -> &str {
        &self.runtime_nonce
    }

    /// Get the expected peer UID.
    pub fn expected_uid(&self) -> u32 {
        self.expected_uid
    }

    /// Full admission check: peer_uid → registry → nonce.
    ///
    /// Checks are short-circuit: first failing check returns immediately.
    pub fn check_admission(&self, peer_uid: u32, source_id: &str, nonce: &str) -> AdmissionResult {
        // 1. Peer UID check
        if peer_uid != self.expected_uid {
            return AdmissionResult::Rejected(RejectionReason::PeerUidMismatch {
                expected: self.expected_uid,
                actual: peer_uid,
            });
        }

        // 2. Source registry check
        if !self.registered_sources.contains(source_id) {
            return AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: source_id.to_owned(),
            });
        }

        // 3. Runtime nonce check
        if nonce != self.runtime_nonce {
            return AdmissionResult::Rejected(RejectionReason::NonceMismatch {
                expected: self.runtime_nonce.clone(),
                actual: nonce.to_owned(),
            });
        }

        AdmissionResult::Admitted
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a guard with uid=1000, nonce="abc123", one registered source "src-a".
    fn make_guard() -> TrustGuard {
        let mut guard = TrustGuard::new(1000, "abc123".to_owned());
        guard.register_source("src-a");
        guard
    }

    // ── 1. admission_all_pass ───────────────────────────────────────

    #[test]
    fn admission_all_pass() {
        let guard = make_guard();
        let result = guard.check_admission(1000, "src-a", "abc123");
        assert_eq!(result, AdmissionResult::Admitted);
    }

    // ── 2. peer_uid_mismatch_rejected ───────────────────────────────

    #[test]
    fn peer_uid_mismatch_rejected() {
        let guard = make_guard();
        let result = guard.check_admission(9999, "src-a", "abc123");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::PeerUidMismatch {
                expected: 1000,
                actual: 9999,
            })
        );
    }

    // ── 3. source_not_registered_rejected ───────────────────────────

    #[test]
    fn source_not_registered_rejected() {
        let guard = make_guard();
        let result = guard.check_admission(1000, "unknown-src", "abc123");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: "unknown-src".to_owned(),
            })
        );
    }

    // ── 4. nonce_mismatch_rejected ──────────────────────────────────

    #[test]
    fn nonce_mismatch_rejected() {
        let guard = make_guard();
        let result = guard.check_admission(1000, "src-a", "wrong-nonce");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::NonceMismatch {
                expected: "abc123".to_owned(),
                actual: "wrong-nonce".to_owned(),
            })
        );
    }

    // ── 5. check_order_uid_first ────────────────────────────────────

    #[test]
    fn check_order_uid_first() {
        // All three checks would fail, but UID should be reported first.
        let guard = make_guard();
        let result = guard.check_admission(9999, "unknown-src", "wrong-nonce");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::PeerUidMismatch {
                expected: 1000,
                actual: 9999,
            })
        );
    }

    // ── 6. check_order_registry_before_nonce ────────────────────────

    #[test]
    fn check_order_registry_before_nonce() {
        // UID passes, but both registry and nonce fail → registry reported.
        let guard = make_guard();
        let result = guard.check_admission(1000, "unknown-src", "wrong-nonce");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: "unknown-src".to_owned(),
            })
        );
    }

    // ── 7. register_and_check ───────────────────────────────────────

    #[test]
    fn register_and_check() {
        let mut guard = TrustGuard::new(1000, "nonce-1".to_owned());
        // Not registered yet
        let result = guard.check_admission(1000, "new-src", "nonce-1");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: "new-src".to_owned(),
            })
        );

        // Register and retry
        guard.register_source("new-src");
        let result = guard.check_admission(1000, "new-src", "nonce-1");
        assert_eq!(result, AdmissionResult::Admitted);
    }

    // ── 8. unregister_revokes_access ────────────────────────────────

    #[test]
    fn unregister_revokes_access() {
        let mut guard = make_guard();

        // Admitted initially
        assert_eq!(
            guard.check_admission(1000, "src-a", "abc123"),
            AdmissionResult::Admitted,
        );

        // Unregister
        assert!(guard.unregister_source("src-a"));

        // Now rejected
        let result = guard.check_admission(1000, "src-a", "abc123");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: "src-a".to_owned(),
            })
        );
    }

    // ── 9. unregister_returns_false_for_unknown ─────────────────────

    #[test]
    fn unregister_returns_false_for_unknown() {
        let mut guard = make_guard();
        assert!(!guard.unregister_source("never-registered"));
    }

    // ── 10. is_registered_true_for_known ────────────────────────────

    #[test]
    fn is_registered_true_for_known() {
        let guard = make_guard();
        assert!(guard.is_registered("src-a"));
    }

    // ── 11. is_registered_false_for_unknown ─────────────────────────

    #[test]
    fn is_registered_false_for_unknown() {
        let guard = make_guard();
        assert!(!guard.is_registered("unknown"));
    }

    // ── 12. registered_count ────────────────────────────────────────

    #[test]
    fn registered_count() {
        let mut guard = TrustGuard::new(1000, "n".to_owned());
        assert_eq!(guard.registered_count(), 0);

        guard.register_source("a");
        guard.register_source("b");
        guard.register_source("c");
        assert_eq!(guard.registered_count(), 3);

        guard.unregister_source("b");
        assert_eq!(guard.registered_count(), 2);
    }

    // ── 13. rotate_nonce_invalidates_old ────────────────────────────

    #[test]
    fn rotate_nonce_invalidates_old() {
        let mut guard = make_guard();

        // Old nonce works
        assert_eq!(
            guard.check_admission(1000, "src-a", "abc123"),
            AdmissionResult::Admitted,
        );

        // Rotate
        guard.rotate_nonce("new-nonce-456".to_owned());

        // Old nonce fails
        let result = guard.check_admission(1000, "src-a", "abc123");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::NonceMismatch {
                expected: "new-nonce-456".to_owned(),
                actual: "abc123".to_owned(),
            })
        );
    }

    // ── 14. rotate_nonce_new_works ──────────────────────────────────

    #[test]
    fn rotate_nonce_new_works() {
        let mut guard = make_guard();
        guard.rotate_nonce("new-nonce-456".to_owned());

        let result = guard.check_admission(1000, "src-a", "new-nonce-456");
        assert_eq!(result, AdmissionResult::Admitted);
    }

    // ── 15. empty_guard_rejects_all_sources ─────────────────────────

    #[test]
    fn empty_guard_rejects_all_sources() {
        let guard = TrustGuard::new(1000, "nonce".to_owned());

        let result = guard.check_admission(1000, "any-source", "nonce");
        assert_eq!(
            result,
            AdmissionResult::Rejected(RejectionReason::SourceNotRegistered {
                source_id: "any-source".to_owned(),
            })
        );
    }
}
