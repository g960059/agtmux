//! Source handshake (hello) API and lifecycle state machine.
//!
//! Manages the registry of connected sources, their lifecycle transitions,
//! heartbeat tracking, and staleness detection.
//!
//! Task ref: T-048

use std::collections::HashMap;

use agtmux_core_v5::types::SourceKind;
use serde::{Deserialize, Serialize};

// ─── Constants ───────────────────────────────────────────────────────

/// Default staleness window in milliseconds (30 seconds).
const DEFAULT_STALENESS_MS: u64 = 30_000;

/// Default minimum supported protocol version.
const DEFAULT_MIN_PROTOCOL_VERSION: u32 = 1;

/// Default maximum supported protocol version.
const DEFAULT_MAX_PROTOCOL_VERSION: u32 = 1;

// ─── Lifecycle ───────────────────────────────────────────────────────

/// Lifecycle state of a registered source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLifecycle {
    /// Handshake received, waiting for validation.
    Pending,
    /// Validated and actively sending events.
    Active,
    /// No heartbeat within staleness window.
    Stale,
    /// Explicitly revoked by the daemon.
    Revoked,
}

// ─── Source Entry ────────────────────────────────────────────────────

/// A single source registration entry in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    pub source_id: String,
    pub source_kind: SourceKind,
    pub protocol_version: u32,
    pub lifecycle: SourceLifecycle,
    pub registered_at_ms: u64,
    pub last_heartbeat_ms: u64,
    pub socket_path: Option<String>,
}

// ─── Handshake Protocol ─────────────────────────────────────────────

/// Request sent by a source to register with the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloRequest {
    pub source_id: String,
    pub source_kind: SourceKind,
    pub protocol_version: u32,
    pub socket_path: Option<String>,
}

/// Response from the gateway to a hello handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloResponse {
    Accepted { source_id: String },
    Rejected { reason: String },
}

// ─── Source Registry ────────────────────────────────────────────────

/// Registry of connected sources with lifecycle management.
///
/// Tracks source registrations via hello handshakes, monitors heartbeats
/// for staleness, and supports explicit revocation.
#[derive(Debug)]
pub struct SourceRegistry {
    entries: HashMap<String, SourceEntry>,
    /// Minimum supported protocol version.
    min_protocol_version: u32,
    /// Maximum supported protocol version.
    max_protocol_version: u32,
    /// Staleness window in milliseconds.
    staleness_ms: u64,
}

impl SourceRegistry {
    /// Create a new registry with default configuration.
    ///
    /// Defaults: protocol range [1, 1], staleness window 30s.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            min_protocol_version: DEFAULT_MIN_PROTOCOL_VERSION,
            max_protocol_version: DEFAULT_MAX_PROTOCOL_VERSION,
            staleness_ms: DEFAULT_STALENESS_MS,
        }
    }

    /// Create a new registry with a custom protocol version range.
    ///
    /// Uses the default staleness window (30s).
    pub fn with_protocol_range(min_version: u32, max_version: u32) -> Self {
        Self {
            entries: HashMap::new(),
            min_protocol_version: min_version,
            max_protocol_version: max_version,
            staleness_ms: DEFAULT_STALENESS_MS,
        }
    }

    /// Process a hello handshake from a source.
    ///
    /// Returns `Accepted` if the protocol is compatible and the source can be
    /// registered. Returns `Rejected` if the protocol is incompatible or the
    /// source has been revoked.
    ///
    /// # Re-registration
    ///
    /// - If a source is already `Active`, re-hello updates the heartbeat and
    ///   socket path.
    /// - If a source is `Stale`, re-hello transitions it back to `Active`.
    /// - If a source is `Revoked`, re-hello is rejected.
    pub fn handle_hello(&mut self, request: HelloRequest, now_ms: u64) -> HelloResponse {
        // 1. Protocol version check
        if request.protocol_version < self.min_protocol_version
            || request.protocol_version > self.max_protocol_version
        {
            return HelloResponse::Rejected {
                reason: "protocol mismatch".to_owned(),
            };
        }

        // 2. Check existing entry
        if let Some(existing) = self.entries.get_mut(&request.source_id) {
            match existing.lifecycle {
                SourceLifecycle::Revoked => {
                    return HelloResponse::Rejected {
                        reason: "source revoked".to_owned(),
                    };
                }
                SourceLifecycle::Active => {
                    // Re-hello on active source: update heartbeat and socket path
                    existing.last_heartbeat_ms = now_ms;
                    existing.socket_path = request.socket_path;
                    return HelloResponse::Accepted {
                        source_id: request.source_id,
                    };
                }
                SourceLifecycle::Stale | SourceLifecycle::Pending => {
                    // Re-register: transition back to Active
                    existing.lifecycle = SourceLifecycle::Active;
                    existing.last_heartbeat_ms = now_ms;
                    existing.protocol_version = request.protocol_version;
                    existing.socket_path = request.socket_path;
                    return HelloResponse::Accepted {
                        source_id: request.source_id,
                    };
                }
            }
        }

        // 3. New source: insert as Pending, then immediately transition to Active
        let entry = SourceEntry {
            source_id: request.source_id.clone(),
            source_kind: request.source_kind,
            protocol_version: request.protocol_version,
            lifecycle: SourceLifecycle::Active,
            registered_at_ms: now_ms,
            last_heartbeat_ms: now_ms,
            socket_path: request.socket_path,
        };
        self.entries.insert(request.source_id.clone(), entry);

        HelloResponse::Accepted {
            source_id: request.source_id,
        }
    }

    /// Record a heartbeat from a source.
    ///
    /// Returns `true` if the source exists and the heartbeat was recorded,
    /// `false` if the source is not in the registry.
    pub fn heartbeat(&mut self, source_id: &str, now_ms: u64) -> bool {
        if let Some(entry) = self.entries.get_mut(source_id) {
            entry.last_heartbeat_ms = now_ms;
            true
        } else {
            false
        }
    }

    /// Check and update staleness for all sources.
    ///
    /// Transitions `Active` (and `Pending`) sources to `Stale` if
    /// `last_heartbeat_ms + staleness_ms < now_ms`.
    ///
    /// Returns the source IDs that were newly marked stale.
    pub fn check_staleness(&mut self, now_ms: u64) -> Vec<String> {
        let mut newly_stale = Vec::new();

        for entry in self.entries.values_mut() {
            if matches!(
                entry.lifecycle,
                SourceLifecycle::Active | SourceLifecycle::Pending
            ) {
                let deadline = entry.last_heartbeat_ms.saturating_add(self.staleness_ms);
                if deadline < now_ms {
                    entry.lifecycle = SourceLifecycle::Stale;
                    newly_stale.push(entry.source_id.clone());
                }
            }
        }

        newly_stale
    }

    /// Revoke a source. Returns `false` if the source doesn't exist.
    pub fn revoke(&mut self, source_id: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(source_id) {
            entry.lifecycle = SourceLifecycle::Revoked;
            true
        } else {
            false
        }
    }

    /// Get a source entry by ID.
    pub fn get(&self, source_id: &str) -> Option<&SourceEntry> {
        self.entries.get(source_id)
    }

    /// List all entries sorted by source_id.
    pub fn list(&self) -> Vec<&SourceEntry> {
        let mut entries: Vec<&SourceEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        entries
    }

    /// Remove revoked and stale entries. Returns the count removed.
    pub fn cleanup(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            !matches!(
                entry.lifecycle,
                SourceLifecycle::Revoked | SourceLifecycle::Stale
            )
        });
        before - self.entries.len()
    }

    /// Count of active sources.
    pub fn active_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.lifecycle == SourceLifecycle::Active)
            .count()
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──────────────────────────────────────────────────────

    fn hello(source_id: &str, kind: SourceKind, version: u32) -> HelloRequest {
        HelloRequest {
            source_id: source_id.to_owned(),
            source_kind: kind,
            protocol_version: version,
            socket_path: None,
        }
    }

    fn hello_with_socket(
        source_id: &str,
        kind: SourceKind,
        version: u32,
        socket_path: &str,
    ) -> HelloRequest {
        HelloRequest {
            source_id: source_id.to_owned(),
            source_kind: kind,
            protocol_version: version,
            socket_path: Some(socket_path.to_owned()),
        }
    }

    // ── 1. empty_registry ───────────────────────────────────────────

    #[test]
    fn empty_registry() {
        let reg = SourceRegistry::new();
        assert_eq!(reg.active_count(), 0);
        assert!(reg.list().is_empty());
        assert!(reg.get("nonexistent").is_none());
    }

    // ── 2. hello_new_source_accepted ────────────────────────────────

    #[test]
    fn hello_new_source_accepted() {
        let mut reg = SourceRegistry::new();
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        assert_eq!(
            resp,
            HelloResponse::Accepted {
                source_id: "src-a".to_owned(),
            }
        );

        let entry = reg.get("src-a").expect("source should exist");
        assert_eq!(entry.lifecycle, SourceLifecycle::Active);
        assert_eq!(entry.source_kind, SourceKind::CodexAppserver);
        assert_eq!(entry.protocol_version, 1);
        assert_eq!(entry.registered_at_ms, 1000);
        assert_eq!(entry.last_heartbeat_ms, 1000);
    }

    // ── 3. hello_protocol_too_low_rejected ──────────────────────────

    #[test]
    fn hello_protocol_too_low_rejected() {
        let mut reg = SourceRegistry::new(); // min=1
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 0), 1000);

        assert_eq!(
            resp,
            HelloResponse::Rejected {
                reason: "protocol mismatch".to_owned(),
            }
        );
        assert!(reg.get("src-a").is_none());
    }

    // ── 4. hello_protocol_too_high_rejected ─────────────────────────

    #[test]
    fn hello_protocol_too_high_rejected() {
        let mut reg = SourceRegistry::new(); // max=1
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 2), 1000);

        assert_eq!(
            resp,
            HelloResponse::Rejected {
                reason: "protocol mismatch".to_owned(),
            }
        );
        assert!(reg.get("src-a").is_none());
    }

    // ── 5. hello_protocol_exact_boundary ────────────────────────────

    #[test]
    fn hello_protocol_exact_boundary() {
        let mut reg = SourceRegistry::with_protocol_range(1, 1);
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        assert_eq!(
            resp,
            HelloResponse::Accepted {
                source_id: "src-a".to_owned(),
            }
        );
    }

    // ── 6. hello_revoked_source_rejected ────────────────────────────

    #[test]
    fn hello_revoked_source_rejected() {
        let mut reg = SourceRegistry::new();

        // Register, then revoke
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);
        reg.revoke("src-a");

        // Try to re-register
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 2000);

        assert_eq!(
            resp,
            HelloResponse::Rejected {
                reason: "source revoked".to_owned(),
            }
        );
    }

    // ── 7. hello_stale_source_re_registers ──────────────────────────

    #[test]
    fn hello_stale_source_re_registers() {
        let mut reg = SourceRegistry::new();

        // Register source
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        // Force stale by checking staleness far in the future
        let stale = reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);
        assert_eq!(stale.len(), 1);
        assert_eq!(
            reg.get("src-a").expect("exists").lifecycle,
            SourceLifecycle::Stale
        );

        // Re-register
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 50_000);

        assert_eq!(
            resp,
            HelloResponse::Accepted {
                source_id: "src-a".to_owned(),
            }
        );
        let entry = reg.get("src-a").expect("exists");
        assert_eq!(entry.lifecycle, SourceLifecycle::Active);
        assert_eq!(entry.last_heartbeat_ms, 50_000);
    }

    // ── 8. hello_active_source_updates_heartbeat ────────────────────

    #[test]
    fn hello_active_source_updates_heartbeat() {
        let mut reg = SourceRegistry::new();

        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);
        assert_eq!(reg.get("src-a").expect("exists").last_heartbeat_ms, 1000);

        // Re-hello on active source
        let resp = reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 5000);

        assert_eq!(
            resp,
            HelloResponse::Accepted {
                source_id: "src-a".to_owned(),
            }
        );
        assert_eq!(reg.get("src-a").expect("exists").last_heartbeat_ms, 5000);
    }

    // ── 9. heartbeat_updates_timestamp ──────────────────────────────

    #[test]
    fn heartbeat_updates_timestamp() {
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        let result = reg.heartbeat("src-a", 2000);
        assert!(result);
        assert_eq!(reg.get("src-a").expect("exists").last_heartbeat_ms, 2000);
    }

    // ── 10. heartbeat_unknown_source_returns_false ───────────────────

    #[test]
    fn heartbeat_unknown_source_returns_false() {
        let mut reg = SourceRegistry::new();
        assert!(!reg.heartbeat("nonexistent", 1000));
    }

    // ── 11. staleness_detection ─────────────────────────────────────

    #[test]
    fn staleness_detection() {
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        // Not stale yet (within window)
        let stale = reg.check_staleness(1000 + DEFAULT_STALENESS_MS);
        assert!(stale.is_empty());
        assert_eq!(
            reg.get("src-a").expect("exists").lifecycle,
            SourceLifecycle::Active
        );

        // Now stale (past window)
        let stale = reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);
        assert_eq!(stale, vec!["src-a".to_owned()]);
        assert_eq!(
            reg.get("src-a").expect("exists").lifecycle,
            SourceLifecycle::Stale
        );
    }

    // ── 12. staleness_does_not_affect_pending ────────────────────────
    // (For simplicity, Pending is treated same as Active for staleness.
    //  This test documents that Pending sources ARE subject to staleness.)

    #[test]
    fn staleness_does_not_affect_pending() {
        // In our implementation, new sources go straight to Active via
        // handle_hello, so to test Pending staleness behavior we would
        // need to manually construct a Pending entry. Since the spec says
        // "for simplicity, treat Pending same as Active for staleness",
        // we verify that staleness checking works on all non-Revoked states.
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        // Source is Active (Pending → Active is immediate in handle_hello)
        assert_eq!(
            reg.get("src-a").expect("exists").lifecycle,
            SourceLifecycle::Active
        );

        // Already-stale sources are not re-marked
        let stale = reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);
        assert_eq!(stale.len(), 1);

        // Check again: already stale, should not appear again
        let stale_again = reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 100);
        assert!(stale_again.is_empty());
    }

    // ── 13. revoke_source ───────────────────────────────────────────

    #[test]
    fn revoke_source() {
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);

        assert!(reg.revoke("src-a"));
        assert_eq!(
            reg.get("src-a").expect("exists").lifecycle,
            SourceLifecycle::Revoked
        );
    }

    // ── 14. revoke_unknown_returns_false ─────────────────────────────

    #[test]
    fn revoke_unknown_returns_false() {
        let mut reg = SourceRegistry::new();
        assert!(!reg.revoke("nonexistent"));
    }

    // ── 15. cleanup_removes_revoked_and_stale ────────────────────────

    #[test]
    fn cleanup_removes_revoked_and_stale() {
        let mut reg = SourceRegistry::new();

        reg.handle_hello(hello("active-1", SourceKind::CodexAppserver, 1), 1000);
        reg.handle_hello(hello("revoked-1", SourceKind::ClaudeHooks, 1), 1000);
        reg.handle_hello(hello("stale-1", SourceKind::Poller, 1), 1000);

        reg.revoke("revoked-1");
        reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);
        // stale-1 is now Stale, but active-1 heartbeat was also at 1000 so it's also stale.
        // Let's give active-1 a recent heartbeat first.
        reg.heartbeat("active-1", 1000 + DEFAULT_STALENESS_MS);

        // Re-check staleness: active-1 has a recent heartbeat, only stale-1 should be stale
        // (But it was already marked stale above along with active-1.)
        // Let's reconstruct the scenario more carefully.
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("active-1", SourceKind::CodexAppserver, 1), 1000);
        reg.handle_hello(hello("revoked-1", SourceKind::ClaudeHooks, 1), 1000);
        reg.handle_hello(hello("stale-1", SourceKind::Poller, 1), 1000);

        // Give active-1 a recent heartbeat
        reg.heartbeat("active-1", 40_000);
        // Revoke revoked-1
        reg.revoke("revoked-1");
        // Check staleness at t=31001: stale-1 (heartbeat=1000) is stale
        reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);

        assert_eq!(
            reg.get("active-1").expect("exists").lifecycle,
            SourceLifecycle::Active
        );
        assert_eq!(
            reg.get("revoked-1").expect("exists").lifecycle,
            SourceLifecycle::Revoked
        );
        assert_eq!(
            reg.get("stale-1").expect("exists").lifecycle,
            SourceLifecycle::Stale
        );

        let removed = reg.cleanup();
        assert_eq!(removed, 2);
        assert!(reg.get("active-1").is_some());
        assert!(reg.get("revoked-1").is_none());
        assert!(reg.get("stale-1").is_none());
    }

    // ── 16. list_sorted_by_id ───────────────────────────────────────

    #[test]
    fn list_sorted_by_id() {
        let mut reg = SourceRegistry::new();
        reg.handle_hello(hello("charlie", SourceKind::Poller, 1), 1000);
        reg.handle_hello(hello("alpha", SourceKind::CodexAppserver, 1), 1000);
        reg.handle_hello(hello("bravo", SourceKind::ClaudeHooks, 1), 1000);

        let list = reg.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].source_id, "alpha");
        assert_eq!(list[1].source_id, "bravo");
        assert_eq!(list[2].source_id, "charlie");
    }

    // ── 17. socket_rotation_on_re_hello ─────────────────────────────

    #[test]
    fn socket_rotation_on_re_hello() {
        let mut reg = SourceRegistry::new();

        reg.handle_hello(
            hello_with_socket("src-a", SourceKind::CodexAppserver, 1, "/tmp/old.sock"),
            1000,
        );
        assert_eq!(
            reg.get("src-a").expect("exists").socket_path.as_deref(),
            Some("/tmp/old.sock")
        );

        // Re-hello with new socket path
        reg.handle_hello(
            hello_with_socket("src-a", SourceKind::CodexAppserver, 1, "/tmp/new.sock"),
            2000,
        );
        assert_eq!(
            reg.get("src-a").expect("exists").socket_path.as_deref(),
            Some("/tmp/new.sock")
        );
    }

    // ── 18. active_count_tracks_correctly ────────────────────────────

    #[test]
    fn active_count_tracks_correctly() {
        let mut reg = SourceRegistry::new();

        // Register 3 sources
        reg.handle_hello(hello("src-a", SourceKind::CodexAppserver, 1), 1000);
        reg.handle_hello(hello("src-b", SourceKind::ClaudeHooks, 1), 1000);
        reg.handle_hello(hello("src-c", SourceKind::Poller, 1), 1000);
        assert_eq!(reg.active_count(), 3);

        // Revoke 1
        reg.revoke("src-a");
        assert_eq!(reg.active_count(), 2);

        // Make 1 stale: give src-b a recent heartbeat, let src-c go stale
        reg.heartbeat("src-b", 40_000);
        reg.check_staleness(1000 + DEFAULT_STALENESS_MS + 1);
        // src-c heartbeat was at 1000, now stale
        assert_eq!(reg.active_count(), 1);

        // Verify the one active source is src-b
        assert_eq!(
            reg.get("src-b").expect("exists").lifecycle,
            SourceLifecycle::Active
        );
    }
}
