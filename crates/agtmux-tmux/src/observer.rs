//! Pane topology observer.
//!
//! [`PaneObserver`] maintains a snapshot of the known tmux pane set and,
//! given a fresh `Vec<RawPane>` from `list_panes()`, computes the diff as
//! a list of [`PaneChange`] events.
//!
//! This module is intentionally **synchronous** — it performs no I/O.  The
//! caller is responsible for polling tmux and feeding the results in.

use agtmux_core::types::RawPane;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// PaneChange
// ---------------------------------------------------------------------------

/// A single topology mutation detected between two `list_panes()` snapshots.
#[derive(Debug, Clone)]
pub enum PaneChange {
    /// A pane appeared that was not present in the previous snapshot.
    Added(RawPane),
    /// A previously known pane is no longer present.
    Removed(String), // pane_id
    /// A pane still exists but one or more observed fields changed.
    Updated {
        pane_id: String,
        old: RawPane,
        new: RawPane,
    },
}

// ---------------------------------------------------------------------------
// PaneObserver
// ---------------------------------------------------------------------------

/// Tracks the known set of tmux panes and computes diffs.
pub struct PaneObserver {
    known_panes: HashMap<String, RawPane>,
}

impl PaneObserver {
    /// Create an observer with an empty initial state.
    pub fn new() -> Self {
        Self {
            known_panes: HashMap::new(),
        }
    }

    /// Compute the diff between the internal state and `current`.
    ///
    /// After this call, the internal state is replaced by `current`.
    ///
    /// The returned vec contains:
    /// - [`PaneChange::Added`] for panes present in `current` but not in the
    ///   previous snapshot.
    /// - [`PaneChange::Removed`] for panes in the previous snapshot but absent
    ///   from `current`.
    /// - [`PaneChange::Updated`] for panes present in both snapshots where
    ///   `current_cmd`, `pane_title`, or `is_active` differ.
    pub fn diff(&mut self, current: Vec<RawPane>) -> Vec<PaneChange> {
        let mut changes = Vec::new();

        // Build a map from the incoming snapshot.
        let mut current_map: HashMap<String, RawPane> = HashMap::with_capacity(current.len());
        for pane in current {
            current_map.insert(pane.pane_id.clone(), pane);
        }

        // Detect Removed and Updated.
        for (id, old) in &self.known_panes {
            match current_map.get(id) {
                None => {
                    changes.push(PaneChange::Removed(id.clone()));
                }
                Some(new) => {
                    if pane_fields_changed(old, new) {
                        changes.push(PaneChange::Updated {
                            pane_id: id.clone(),
                            old: old.clone(),
                            new: new.clone(),
                        });
                    }
                }
            }
        }

        // Detect Added.
        for (id, pane) in &current_map {
            if !self.known_panes.contains_key(id) {
                changes.push(PaneChange::Added(pane.clone()));
            }
        }

        // Replace known state.
        self.known_panes = current_map;

        changes
    }

    /// Reference to the current known pane set.
    pub fn known_panes(&self) -> &HashMap<String, RawPane> {
        &self.known_panes
    }
}

impl Default for PaneObserver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compare the fields we care about for topology change detection.
///
/// We intentionally do NOT derive `PartialEq` on `RawPane` (it lives in
/// `agtmux-core`).  Instead we manually compare the subset of fields that
/// constitute a meaningful change.
fn pane_fields_changed(old: &RawPane, new: &RawPane) -> bool {
    old.current_cmd != new.current_cmd
        || old.pane_title != new.pane_title
        || old.is_active != new.is_active
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a `RawPane` with sensible defaults.
    fn make_pane(id: &str, cmd: &str, title: &str, active: bool) -> RawPane {
        RawPane {
            pane_id: id.to_owned(),
            session_name: "main".to_owned(),
            window_id: "@0".to_owned(),
            window_name: "bash".to_owned(),
            pane_title: title.to_owned(),
            current_cmd: cmd.to_owned(),
            width: 80,
            height: 24,
            is_active: active,
        }
    }

    // ---------------------------------------------------------------
    // Empty -> Some panes = all Added
    // ---------------------------------------------------------------

    #[test]
    fn initial_diff_returns_all_added() {
        let mut obs = PaneObserver::new();
        let panes = vec![
            make_pane("%0", "bash", "pane0", true),
            make_pane("%1", "vim", "pane1", false),
        ];

        let changes = obs.diff(panes);

        let added: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                PaneChange::Added(p) => Some(p.pane_id.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(added.len(), 2);
        assert!(added.contains(&"%0"));
        assert!(added.contains(&"%1"));
    }

    // ---------------------------------------------------------------
    // Some panes -> Empty = all Removed
    // ---------------------------------------------------------------

    #[test]
    fn all_removed_when_current_is_empty() {
        let mut obs = PaneObserver::new();
        obs.diff(vec![
            make_pane("%0", "bash", "pane0", true),
            make_pane("%1", "vim", "pane1", false),
        ]);

        let changes = obs.diff(vec![]);

        let removed: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                PaneChange::Removed(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"%0"));
        assert!(removed.contains(&"%1"));
    }

    // ---------------------------------------------------------------
    // No changes
    // ---------------------------------------------------------------

    #[test]
    fn no_changes_when_identical() {
        let mut obs = PaneObserver::new();
        let panes = vec![make_pane("%0", "bash", "pane0", true)];

        obs.diff(panes.clone());
        let changes = obs.diff(panes);

        assert!(changes.is_empty(), "expected no changes, got: {changes:?}");
    }

    // ---------------------------------------------------------------
    // Updated: current_cmd changed
    // ---------------------------------------------------------------

    #[test]
    fn updated_on_cmd_change() {
        let mut obs = PaneObserver::new();
        obs.diff(vec![make_pane("%0", "bash", "t", true)]);

        let changes = obs.diff(vec![make_pane("%0", "python", "t", true)]);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            PaneChange::Updated { pane_id, old, new } => {
                assert_eq!(pane_id, "%0");
                assert_eq!(old.current_cmd, "bash");
                assert_eq!(new.current_cmd, "python");
            }
            other => panic!("expected Updated, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Updated: pane_title changed
    // ---------------------------------------------------------------

    #[test]
    fn updated_on_title_change() {
        let mut obs = PaneObserver::new();
        obs.diff(vec![make_pane("%0", "bash", "old-title", true)]);

        let changes = obs.diff(vec![make_pane("%0", "bash", "new-title", true)]);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            PaneChange::Updated { pane_id, old, new } => {
                assert_eq!(pane_id, "%0");
                assert_eq!(old.pane_title, "old-title");
                assert_eq!(new.pane_title, "new-title");
            }
            other => panic!("expected Updated, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Updated: is_active changed
    // ---------------------------------------------------------------

    #[test]
    fn updated_on_active_change() {
        let mut obs = PaneObserver::new();
        obs.diff(vec![make_pane("%0", "bash", "t", true)]);

        let changes = obs.diff(vec![make_pane("%0", "bash", "t", false)]);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            PaneChange::Updated { pane_id, old, new } => {
                assert_eq!(pane_id, "%0");
                assert!(old.is_active);
                assert!(!new.is_active);
            }
            other => panic!("expected Updated, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Mixed: Added + Removed + Updated in single diff
    // ---------------------------------------------------------------

    #[test]
    fn mixed_add_remove_update() {
        let mut obs = PaneObserver::new();

        // Initial state: %0 and %1
        obs.diff(vec![
            make_pane("%0", "bash", "t0", true),
            make_pane("%1", "vim", "t1", false),
        ]);

        // New state: %0 updated, %1 removed, %2 added
        let changes = obs.diff(vec![
            make_pane("%0", "htop", "t0", true), // cmd changed
            make_pane("%2", "node", "t2", false), // new
        ]);

        let mut has_updated = false;
        let mut has_removed = false;
        let mut has_added = false;

        for c in &changes {
            match c {
                PaneChange::Updated { pane_id, .. } if pane_id == "%0" => {
                    has_updated = true;
                }
                PaneChange::Removed(id) if id == "%1" => {
                    has_removed = true;
                }
                PaneChange::Added(p) if p.pane_id == "%2" => {
                    has_added = true;
                }
                other => panic!("unexpected change: {other:?}"),
            }
        }

        assert!(has_updated, "expected %0 Updated");
        assert!(has_removed, "expected %1 Removed");
        assert!(has_added, "expected %2 Added");
    }

    // ---------------------------------------------------------------
    // Non-tracked field changes are ignored
    // ---------------------------------------------------------------

    #[test]
    fn non_tracked_field_changes_ignored() {
        let mut obs = PaneObserver::new();
        obs.diff(vec![make_pane("%0", "bash", "t", true)]);

        // Change width/height/session_name — should NOT trigger Updated.
        let mut pane = make_pane("%0", "bash", "t", true);
        pane.width = 120;
        pane.height = 40;
        pane.session_name = "other".to_owned();
        pane.window_id = "@5".to_owned();
        pane.window_name = "zsh".to_owned();

        let changes = obs.diff(vec![pane]);
        assert!(
            changes.is_empty(),
            "non-tracked field changes should be silent, got: {changes:?}",
        );
    }

    // ---------------------------------------------------------------
    // known_panes reflects last diff state
    // ---------------------------------------------------------------

    #[test]
    fn known_panes_updated_after_diff() {
        let mut obs = PaneObserver::new();
        assert!(obs.known_panes().is_empty());

        obs.diff(vec![make_pane("%0", "bash", "t", true)]);
        assert_eq!(obs.known_panes().len(), 1);
        assert!(obs.known_panes().contains_key("%0"));

        obs.diff(vec![]);
        assert!(obs.known_panes().is_empty());
    }

    // ---------------------------------------------------------------
    // Successive diffs are independent
    // ---------------------------------------------------------------

    #[test]
    fn successive_diffs_are_independent() {
        let mut obs = PaneObserver::new();

        // Diff 1: add %0
        let c1 = obs.diff(vec![make_pane("%0", "bash", "t", true)]);
        assert_eq!(c1.len(), 1);

        // Diff 2: no change
        let c2 = obs.diff(vec![make_pane("%0", "bash", "t", true)]);
        assert!(c2.is_empty());

        // Diff 3: update
        let c3 = obs.diff(vec![make_pane("%0", "vim", "t", true)]);
        assert_eq!(c3.len(), 1);

        // Diff 4: still vim — no change
        let c4 = obs.diff(vec![make_pane("%0", "vim", "t", true)]);
        assert!(c4.is_empty());
    }

    // ---------------------------------------------------------------
    // Default trait
    // ---------------------------------------------------------------

    #[test]
    fn default_creates_empty_observer() {
        let obs = PaneObserver::default();
        assert!(obs.known_panes().is_empty());
    }
}
