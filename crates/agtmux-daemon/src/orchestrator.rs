use agtmux_core::attn::derive_attention_state;
use agtmux_core::engine::{Engine, EngineConfig, ResolvedActivity};
use agtmux_core::source::{SourceEvent, TopologyEvent};
use agtmux_core::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

use crate::server::{PaneInfo, SharedState};

/// State tracked per pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneState {
    pub pane_id: String,
    pub provider: Option<Provider>,
    pub provider_confidence: f64,
    pub activity: ResolvedActivity,
    pub attention: AttentionResult,
    pub last_event_type: String,
    pub updated_at: DateTime<Utc>,
}

/// Notification sent to clients when state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StateNotification {
    StateChanged { pane_id: String, state: PaneState },
    PaneAdded { pane_id: String },
    PaneRemoved { pane_id: String },
}

pub struct Orchestrator {
    engine: Engine,
    /// Per-pane evidence window. Key = pane_id.
    evidence_windows: HashMap<String, Vec<Evidence>>,
    /// Current resolved state per pane.
    pane_states: HashMap<String, PaneState>,
    /// Receives events from all sources.
    source_rx: mpsc::Receiver<SourceEvent>,
    /// Broadcasts state changes to all clients.
    notify_tx: broadcast::Sender<StateNotification>,
    /// Shared state written by orchestrator, read by server for list_panes API.
    shared_state: SharedState,
}

/// Serialize an enum variant to its serde snake_case string representation.
fn serde_variant_name<T: Serialize>(value: &T) -> String {
    // serde_json serializes a unit enum variant as a JSON string, e.g. `"waiting_input"`.
    // We strip the surrounding quotes to get the raw string.
    let json = serde_json::to_string(value).unwrap_or_default();
    json.trim_matches('"').to_string()
}

impl From<&PaneState> for PaneInfo {
    fn from(ps: &PaneState) -> Self {
        PaneInfo {
            pane_id: ps.pane_id.clone(),
            // PaneMeta fields are not yet tracked in PaneState — use empty defaults.
            session_name: String::new(),
            window_id: String::new(),
            pane_title: String::new(),
            current_cmd: String::new(),
            provider: ps.provider.as_ref().map(|p| serde_variant_name(p)),
            provider_confidence: ps.provider_confidence,
            activity_state: serde_variant_name(&ps.activity.state),
            activity_confidence: ps.activity.confidence,
            activity_source: serde_variant_name(&ps.activity.source),
            attention_state: serde_variant_name(&ps.attention.state),
            attention_reason: ps.attention.reason.clone(),
            attention_since: ps.attention.since.map(|dt| dt.to_rfc3339()),
            updated_at: ps.updated_at.to_rfc3339(),
        }
    }
}

impl Orchestrator {
    pub fn new(
        source_rx: mpsc::Receiver<SourceEvent>,
        notify_tx: broadcast::Sender<StateNotification>,
        shared_state: SharedState,
    ) -> Self {
        Self {
            engine: Engine::new(EngineConfig::default()),
            evidence_windows: HashMap::new(),
            pane_states: HashMap::new(),
            source_rx,
            notify_tx,
            shared_state,
        }
    }

    /// Main event loop. Runs until the source channel is closed.
    pub async fn run(&mut self) {
        info!("orchestrator: event loop started");
        while let Some(event) = self.source_rx.recv().await {
            self.handle_event(event);
        }
        info!("orchestrator: source channel closed, shutting down");
    }

    fn handle_event(&mut self, event: SourceEvent) {
        match event {
            SourceEvent::Evidence { pane_id, evidence } => {
                debug!(pane_id = %pane_id, count = evidence.len(), "ingesting evidence");
                self.ingest_evidence(&pane_id, evidence);
                self.evaluate_pane(&pane_id);
            }
            SourceEvent::RawSignal {
                pane_id,
                event_type,
                ..
            } => {
                // Store event_type for attention derivation.
                // Full EventNormalizer pipeline will be wired in Phase 2.
                debug!(pane_id = %pane_id, event_type = %event_type, "raw signal received");
                if let Some(state) = self.pane_states.get_mut(&pane_id) {
                    state.last_event_type = event_type;
                }
            }
            SourceEvent::TopologyChange(topo) => {
                self.handle_topology(topo);
            }
        }
    }

    /// Add evidence to a pane's window, superseding old evidence from same (provider, source).
    fn ingest_evidence(&mut self, pane_id: &str, new_evidence: Vec<Evidence>) {
        let window = self
            .evidence_windows
            .entry(pane_id.to_string())
            .or_default();
        for ev in new_evidence {
            // Supersede: remove old evidence from same provider+source
            window.retain(|old| !(old.provider == ev.provider && old.source == ev.source));
            window.push(ev);
        }
    }

    /// Re-evaluate a pane's state and broadcast if changed.
    fn evaluate_pane(&mut self, pane_id: &str) {
        let now = Utc::now();
        let window = match self.evidence_windows.get(pane_id) {
            Some(w) => w,
            None => return,
        };

        let resolved = self.engine.resolve(window, now);

        let last_event_type = self
            .pane_states
            .get(pane_id)
            .map(|s| s.last_event_type.clone())
            .unwrap_or_default();

        let attention = derive_attention_state(
            resolved.state,
            &resolved.reason_code,
            &last_event_type,
            now,
        );

        let new_state = PaneState {
            pane_id: pane_id.to_string(),
            provider: self.detect_provider(pane_id),
            provider_confidence: 0.0, // TODO: track from detection in Phase 2
            activity: resolved.clone(),
            attention,
            last_event_type,
            updated_at: now,
        };

        // Check if state actually changed before broadcasting
        let changed = match self.pane_states.get(pane_id) {
            Some(old) => {
                old.activity.state != new_state.activity.state
                    || old.attention.state != new_state.attention.state
            }
            None => true,
        };

        self.pane_states
            .insert(pane_id.to_string(), new_state.clone());

        if changed {
            debug!(pane_id = %pane_id, state = ?new_state.activity.state, "state changed, broadcasting");
            // Ignore send errors — no subscribers is fine
            let _ = self.notify_tx.send(StateNotification::StateChanged {
                pane_id: pane_id.to_string(),
                state: new_state,
            });
        }

        self.sync_shared_state();
    }

    fn detect_provider(&self, _pane_id: &str) -> Option<Provider> {
        // Provider detection will be wired in Phase 2 when PaneMeta tracking is
        // added to Orchestrator and ProviderDetector instances are loaded here.
        // For now, return None — callers can still infer provider from evidence.
        None
    }

    /// Synchronize the SharedState with current pane_states so the server's
    /// `list_panes` API returns up-to-date data.
    fn sync_shared_state(&self) {
        let pane_infos: Vec<PaneInfo> = self.pane_states.values().map(PaneInfo::from).collect();
        // Use try_write() to avoid blocking the async runtime. Lock contention
        // is minimal since the server only holds the read lock briefly during
        // list_panes, so this should virtually always succeed.
        match self.shared_state.try_write() {
            Ok(mut state) => {
                state.panes = pane_infos;
            }
            Err(_) => {
                debug!("shared state write lock contended, will sync on next evaluation");
            }
        }
    }

    fn handle_topology(&mut self, event: TopologyEvent) {
        match event {
            TopologyEvent::PaneAdded { ref pane_id } => {
                info!(pane_id = %pane_id, "pane added");
                let _ = self.notify_tx.send(StateNotification::PaneAdded {
                    pane_id: pane_id.clone(),
                });
            }
            TopologyEvent::PaneRemoved { ref pane_id } => {
                info!(pane_id = %pane_id, "pane removed");
                self.evidence_windows.remove(pane_id);
                self.pane_states.remove(pane_id);
                let _ = self.notify_tx.send(StateNotification::PaneRemoved {
                    pane_id: pane_id.clone(),
                });
                self.sync_shared_state();
            }
        }
    }

    /// Get current state of all panes. Used by server for list_panes API.
    pub fn get_all_states(&self) -> Vec<PaneState> {
        self.pane_states.values().cloned().collect()
    }

    /// Get state of a specific pane.
    pub fn get_pane_state(&self, pane_id: &str) -> Option<&PaneState> {
        self.pane_states.get(pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::DaemonState;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc, RwLock};

    /// Helper to create the orchestrator and its channels.
    fn create_orchestrator() -> (
        mpsc::Sender<SourceEvent>,
        broadcast::Receiver<StateNotification>,
        Orchestrator,
        SharedState,
    ) {
        let (source_tx, source_rx) = mpsc::channel(64);
        let (notify_tx, notify_rx) = broadcast::channel(64);
        let shared_state: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let orch = Orchestrator::new(source_rx, notify_tx, shared_state.clone());
        (source_tx, notify_rx, orch, shared_state)
    }

    /// Helper to create evidence with given parameters.
    fn make_evidence(
        provider: Provider,
        signal: ActivityState,
        weight: f64,
        confidence: f64,
        source: SourceType,
        reason_code: &str,
    ) -> Evidence {
        Evidence {
            provider,
            kind: EvidenceKind::PollerMatch("test".into()),
            signal,
            weight,
            confidence,
            timestamp: Utc::now(),
            ttl: Duration::from_secs(90),
            source,
            reason_code: reason_code.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Evidence ingestion tests
    // -----------------------------------------------------------------------

    #[test]
    fn ingest_evidence_adds_to_window() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev]);

        assert_eq!(orch.evidence_windows.get("%1").unwrap().len(), 1);
    }

    #[test]
    fn ingest_evidence_supersedes_same_provider_source() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // First evidence: Running from Claude via Hook
        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev1]);
        assert_eq!(orch.evidence_windows["%1"].len(), 1);
        assert_eq!(orch.evidence_windows["%1"][0].signal, ActivityState::Running);

        // Second evidence: WaitingInput from Claude via Hook — should supersede
        let ev2 = make_evidence(
            Provider::Claude,
            ActivityState::WaitingInput,
            0.95,
            0.95,
            SourceType::Hook,
            "waiting_input",
        );
        orch.ingest_evidence("%1", vec![ev2]);
        assert_eq!(orch.evidence_windows["%1"].len(), 1);
        assert_eq!(
            orch.evidence_windows["%1"][0].signal,
            ActivityState::WaitingInput
        );
    }

    #[test]
    fn ingest_evidence_keeps_different_sources() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "hook",
        );
        let ev2 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.7,
            0.8,
            SourceType::Poller,
            "poller",
        );
        orch.ingest_evidence("%1", vec![ev1, ev2]);

        // Both should be retained since they have different sources
        assert_eq!(orch.evidence_windows["%1"].len(), 2);
    }

    #[test]
    fn ingest_evidence_keeps_different_providers() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "",
        );
        let ev2 = make_evidence(
            Provider::Codex,
            ActivityState::Running,
            0.7,
            0.8,
            SourceType::Hook,
            "",
        );
        orch.ingest_evidence("%1", vec![ev1, ev2]);

        // Both should be retained since they have different providers
        assert_eq!(orch.evidence_windows["%1"].len(), 2);
    }

    // -----------------------------------------------------------------------
    // Topology handling tests
    // -----------------------------------------------------------------------

    #[test]
    fn topology_pane_added_broadcasts() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        orch.handle_topology(TopologyEvent::PaneAdded {
            pane_id: "%1".into(),
        });

        let notif = rx.try_recv().expect("should receive notification");
        match notif {
            StateNotification::PaneAdded { pane_id } => assert_eq!(pane_id, "%1"),
            other => panic!("unexpected notification: {:?}", other),
        }
    }

    #[test]
    fn topology_pane_removed_cleans_state() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        // Populate some state first
        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        assert!(orch.evidence_windows.contains_key("%1"));
        assert!(orch.pane_states.contains_key("%1"));

        // Drain the StateChanged notification
        let _ = rx.try_recv();

        // Remove the pane
        orch.handle_topology(TopologyEvent::PaneRemoved {
            pane_id: "%1".into(),
        });

        assert!(!orch.evidence_windows.contains_key("%1"));
        assert!(!orch.pane_states.contains_key("%1"));

        let notif = rx.try_recv().expect("should receive removal notification");
        match notif {
            StateNotification::PaneRemoved { pane_id } => assert_eq!(pane_id, "%1"),
            other => panic!("unexpected notification: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // State evaluation tests
    // -----------------------------------------------------------------------

    #[test]
    fn evaluate_pane_creates_state() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        let ev = make_evidence(
            Provider::Claude,
            ActivityState::WaitingApproval,
            0.95,
            0.95,
            SourceType::Hook,
            "needs_approval",
        );
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        let state = orch.get_pane_state("%1").expect("pane state should exist");
        assert_eq!(state.activity.state, ActivityState::WaitingApproval);
        assert_eq!(
            state.attention.state,
            AttentionState::ActionRequiredApproval
        );

        // Should have broadcast a notification
        let notif = rx.try_recv().expect("should receive state change");
        match notif {
            StateNotification::StateChanged { pane_id, state } => {
                assert_eq!(pane_id, "%1");
                assert_eq!(state.activity.state, ActivityState::WaitingApproval);
            }
            other => panic!("unexpected notification: {:?}", other),
        }
    }

    #[test]
    fn evaluate_pane_no_broadcast_if_unchanged() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev.clone()]);
        orch.evaluate_pane("%1");

        // Drain first notification
        let _ = rx.try_recv();

        // Re-evaluate with same state — should NOT broadcast
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        assert!(
            rx.try_recv().is_err(),
            "should not broadcast when state unchanged"
        );
    }

    #[test]
    fn evaluate_pane_broadcasts_on_state_change() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        // Start with Running
        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev1]);
        orch.evaluate_pane("%1");
        let _ = rx.try_recv(); // drain

        // Transition to Error
        let ev2 = make_evidence(
            Provider::Claude,
            ActivityState::Error,
            1.0,
            0.95,
            SourceType::Hook,
            "error",
        );
        orch.ingest_evidence("%1", vec![ev2]);
        orch.evaluate_pane("%1");

        let notif = rx.try_recv().expect("should broadcast on state change");
        match notif {
            StateNotification::StateChanged { state, .. } => {
                assert_eq!(state.activity.state, ActivityState::Error);
                assert_eq!(state.attention.state, AttentionState::ActionRequiredError);
            }
            other => panic!("unexpected notification: {:?}", other),
        }
    }

    #[test]
    fn evaluate_pane_empty_window_is_noop() {
        let (_tx, mut rx, mut orch, _shared) = create_orchestrator();

        // No evidence, no window entry — should be a no-op
        orch.evaluate_pane("%nonexistent");
        assert!(orch.get_pane_state("%nonexistent").is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn get_all_states_returns_all_panes() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "",
        );
        let ev2 = make_evidence(
            Provider::Codex,
            ActivityState::Idle,
            0.9,
            0.9,
            SourceType::Hook,
            "",
        );
        orch.ingest_evidence("%1", vec![ev1]);
        orch.ingest_evidence("%2", vec![ev2]);
        orch.evaluate_pane("%1");
        orch.evaluate_pane("%2");

        let all = orch.get_all_states();
        assert_eq!(all.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Full event loop integration test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn run_processes_events_until_channel_closed() {
        let (tx, rx) = mpsc::channel::<SourceEvent>(64);
        let (ntx, _nrx) = broadcast::channel::<StateNotification>(64);
        let shared: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let mut orch = Orchestrator::new(rx, ntx, shared.clone());

        // Send events, then close channel
        let ev = make_evidence(
            Provider::Claude,
            ActivityState::WaitingInput,
            0.95,
            0.92,
            SourceType::Hook,
            "waiting_input",
        );
        tx.send(SourceEvent::Evidence {
            pane_id: "%1".into(),
            evidence: vec![ev],
        })
        .await
        .unwrap();

        tx.send(SourceEvent::TopologyChange(TopologyEvent::PaneAdded {
            pane_id: "%2".into(),
        }))
        .await
        .unwrap();

        // Drop sender to close the channel
        drop(tx);

        // Run orchestrator — it should process both events then return
        orch.run().await;

        // Verify the pane state was created
        let state = orch.get_pane_state("%1").expect("pane %1 should exist");
        assert_eq!(state.activity.state, ActivityState::WaitingInput);
    }

    #[test]
    fn raw_signal_updates_last_event_type() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // First create pane state via evidence
        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        // Now send a RawSignal
        orch.handle_event(SourceEvent::RawSignal {
            pane_id: "%1".into(),
            event_type: "tool-execution".into(),
            source: SourceType::Hook,
            payload: "{}".into(),
            timestamp: Utc::now(),
        });

        let state = orch.get_pane_state("%1").unwrap();
        assert_eq!(state.last_event_type, "tool-execution");
    }

    // -----------------------------------------------------------------------
    // SharedState wiring tests
    // -----------------------------------------------------------------------

    #[test]
    fn evaluate_pane_syncs_shared_state() {
        let (_tx, _rx, mut orch, shared) = create_orchestrator();

        let ev = make_evidence(
            Provider::Claude,
            ActivityState::WaitingApproval,
            0.95,
            0.95,
            SourceType::Hook,
            "needs_approval",
        );
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        // SharedState should now contain the pane info
        let state = shared.try_read().expect("should be able to read shared state");
        assert_eq!(state.panes.len(), 1);

        let pane = &state.panes[0];
        assert_eq!(pane.pane_id, "%1");
        assert_eq!(pane.activity_state, "waiting_approval");
        assert_eq!(pane.attention_state, "action_required_approval");
        assert_eq!(pane.activity_source, "hook");
        // The engine applies a hook bonus, so confidence will be >= input confidence
        assert!(pane.activity_confidence > 0.0);
        // PaneMeta fields should be empty defaults
        assert_eq!(pane.session_name, "");
        assert_eq!(pane.window_id, "");
        assert_eq!(pane.pane_title, "");
        assert_eq!(pane.current_cmd, "");
    }

    #[test]
    fn pane_removal_syncs_shared_state() {
        let (_tx, mut rx, mut orch, shared) = create_orchestrator();

        // Add a pane via evidence
        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        orch.ingest_evidence("%1", vec![ev]);
        orch.evaluate_pane("%1");

        // Drain the StateChanged notification
        let _ = rx.try_recv();

        // Verify pane is in shared state
        {
            let state = shared.try_read().unwrap();
            assert_eq!(state.panes.len(), 1);
        }

        // Remove the pane
        orch.handle_topology(TopologyEvent::PaneRemoved {
            pane_id: "%1".into(),
        });

        // SharedState should now be empty
        let state = shared.try_read().unwrap();
        assert_eq!(state.panes.len(), 0);
    }

    #[test]
    fn pane_state_to_pane_info_conversion() {
        let ps = PaneState {
            pane_id: "%42".into(),
            provider: Some(Provider::Codex),
            provider_confidence: 0.88,
            activity: agtmux_core::engine::ResolvedActivity {
                state: ActivityState::Error,
                confidence: 0.95,
                source: SourceType::Poller,
                reason_code: "error_detected".into(),
            },
            attention: AttentionResult {
                state: AttentionState::ActionRequiredError,
                reason: "process crashed".into(),
                since: Some(Utc::now()),
            },
            last_event_type: "error".into(),
            updated_at: Utc::now(),
        };

        let info: PaneInfo = PaneInfo::from(&ps);
        assert_eq!(info.pane_id, "%42");
        assert_eq!(info.provider, Some("codex".to_string()));
        assert_eq!(info.provider_confidence, 0.88);
        assert_eq!(info.activity_state, "error");
        assert_eq!(info.activity_confidence, 0.95);
        assert_eq!(info.activity_source, "poller");
        assert_eq!(info.attention_state, "action_required_error");
        assert_eq!(info.attention_reason, "process crashed");
        assert!(info.attention_since.is_some());
        assert!(!info.updated_at.is_empty());
    }

    #[test]
    fn pane_state_to_pane_info_none_provider() {
        let ps = PaneState {
            pane_id: "%1".into(),
            provider: None,
            provider_confidence: 0.0,
            activity: agtmux_core::engine::ResolvedActivity {
                state: ActivityState::Unknown,
                confidence: 0.5,
                source: SourceType::Poller,
                reason_code: "".into(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: "".into(),
                since: None,
            },
            last_event_type: "".into(),
            updated_at: Utc::now(),
        };

        let info: PaneInfo = PaneInfo::from(&ps);
        assert_eq!(info.provider, None);
        assert_eq!(info.activity_state, "unknown");
        assert_eq!(info.attention_state, "none");
        assert_eq!(info.attention_since, None);
    }

    #[tokio::test]
    async fn run_syncs_shared_state_on_evidence() {
        let (tx, rx) = mpsc::channel::<SourceEvent>(64);
        let (ntx, _nrx) = broadcast::channel::<StateNotification>(64);
        let shared: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let mut orch = Orchestrator::new(rx, ntx, shared.clone());

        let ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        tx.send(SourceEvent::Evidence {
            pane_id: "%1".into(),
            evidence: vec![ev],
        })
        .await
        .unwrap();

        drop(tx);
        orch.run().await;

        // Verify shared state was updated via the event loop
        let state = shared.read().await;
        assert_eq!(state.panes.len(), 1);
        assert_eq!(state.panes[0].pane_id, "%1");
        assert_eq!(state.panes[0].activity_state, "running");
    }
}
