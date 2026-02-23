use agtmux_core::adapt::ProviderDetector;
use agtmux_core::attn::derive_attention_state;
use agtmux_core::engine::{Engine, EngineConfig, ResolvedActivity};
use agtmux_core::source::{SourceEvent, TopologyEvent};
use agtmux_core::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::serde_helpers::{parse_enum, serde_variant_name};
use crate::server::{PaneInfo, SharedState};

/// Minimum confidence threshold for provider detection.
/// Detectors returning a confidence below this value are ignored.
const PROVIDER_DETECTION_THRESHOLD: f64 = 0.5;

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
    /// Provider detectors loaded from TOML definitions.
    detectors: Vec<Box<dyn ProviderDetector>>,
    /// Per-pane evidence window. Key = pane_id.
    evidence_windows: HashMap<String, Vec<Evidence>>,
    /// Cached PaneMeta per pane, populated from evidence context.
    /// Used as input to ProviderDetector::detect().
    pane_metas: HashMap<String, PaneMeta>,
    /// Current resolved state per pane.
    pane_states: HashMap<String, PaneState>,
    /// Receives events from all sources.
    source_rx: mpsc::Receiver<SourceEvent>,
    /// Broadcasts state changes to all clients.
    notify_tx: broadcast::Sender<StateNotification>,
    /// Shared state written by orchestrator, read by server for list_panes API.
    shared_state: SharedState,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
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

/// Reverse conversion for persistence recovery. Lossy: `reason_code` and
/// `last_event_type` are set to empty strings, and PaneMeta fields
/// (session_name, window_id, pane_title, current_cmd) are not carried.
/// These fields will be repopulated by the next evidence cycle.
impl From<&PaneInfo> for PaneState {
    fn from(pi: &PaneInfo) -> Self {
        let provider: Option<Provider> = pi.provider.as_deref().and_then(parse_enum);
        let activity_state: ActivityState =
            parse_enum(&pi.activity_state).unwrap_or(ActivityState::Unknown);
        let activity_source: SourceType =
            parse_enum(&pi.activity_source).unwrap_or(SourceType::Poller);
        let attention_state: AttentionState =
            parse_enum(&pi.attention_state).unwrap_or(AttentionState::None);
        let attention_since: Option<DateTime<Utc>> = pi
            .attention_since
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let updated_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&pi.updated_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        PaneState {
            pane_id: pi.pane_id.clone(),
            provider,
            provider_confidence: pi.provider_confidence,
            activity: ResolvedActivity {
                state: activity_state,
                confidence: pi.activity_confidence,
                source: activity_source,
                reason_code: String::new(),
            },
            attention: AttentionResult {
                state: attention_state,
                reason: pi.attention_reason.clone(),
                since: attention_since,
            },
            last_event_type: String::new(),
            updated_at,
        }
    }
}

impl Orchestrator {
    pub fn new(
        source_rx: mpsc::Receiver<SourceEvent>,
        notify_tx: broadcast::Sender<StateNotification>,
        shared_state: SharedState,
        detectors: Vec<Box<dyn ProviderDetector>>,
    ) -> Self {
        Self::with_cancel(source_rx, notify_tx, shared_state, detectors, CancellationToken::new())
    }

    /// Create an orchestrator with an explicit cancellation token for graceful shutdown.
    pub fn with_cancel(
        source_rx: mpsc::Receiver<SourceEvent>,
        notify_tx: broadcast::Sender<StateNotification>,
        shared_state: SharedState,
        detectors: Vec<Box<dyn ProviderDetector>>,
        cancel: CancellationToken,
    ) -> Self {
        info!(
            detector_count = detectors.len(),
            "orchestrator: loaded provider detectors"
        );
        Self {
            engine: Engine::new(EngineConfig::default()),
            detectors,
            evidence_windows: HashMap::new(),
            pane_metas: HashMap::new(),
            pane_states: HashMap::new(),
            source_rx,
            notify_tx,
            shared_state,
            cancel,
        }
    }

    /// Main event loop. Runs until the source channel is closed or the
    /// cancellation token is triggered.
    pub async fn run(&mut self) {
        info!("orchestrator: event loop started");
        loop {
            tokio::select! {
                event = self.source_rx.recv() => {
                    match event {
                        Some(event) => self.handle_event(event),
                        None => {
                            info!("orchestrator: source channel closed, shutting down");
                            break;
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    info!("orchestrator: cancellation requested, shutting down");
                    break;
                }
            }
        }
    }

    fn handle_event(&mut self, event: SourceEvent) {
        match event {
            SourceEvent::Evidence {
                pane_id,
                evidence,
                meta,
            } => {
                debug!(pane_id = %pane_id, count = evidence.len(), "ingesting evidence");
                if let Some(m) = meta {
                    self.update_pane_meta(&pane_id, m);
                }
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

    /// Remove expired evidence from a pane's window.
    /// Evidence is expired when `now > evidence.timestamp + evidence.ttl`.
    fn expire_evidence(&mut self, pane_id: &str, now: DateTime<Utc>) {
        if let Some(window) = self.evidence_windows.get_mut(pane_id) {
            window.retain(|ev| {
                let ttl_ms = ev.ttl.as_millis() as i64;
                if ttl_ms <= 0 {
                    // Zero TTL means no expiration from this filter; the engine
                    // will apply the default TTL itself.
                    return true;
                }
                let age = now.signed_duration_since(ev.timestamp);
                age.num_milliseconds() <= ttl_ms
            });
        }
    }

    /// Re-evaluate a pane's state and broadcast if changed.
    fn evaluate_pane(&mut self, pane_id: &str) {
        let now = Utc::now();
        self.expire_evidence(pane_id, now);

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

        let (detected_provider, detected_confidence) = self.detect_provider(pane_id);

        let new_state = PaneState {
            pane_id: pane_id.to_string(),
            provider: detected_provider,
            provider_confidence: detected_confidence,
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

    /// Run all registered ProviderDetectors against the cached PaneMeta for this
    /// pane. Returns the provider with the highest confidence above the threshold,
    /// or `(None, 0.0)` if no detector matches.
    fn detect_provider(&self, pane_id: &str) -> (Option<Provider>, f64) {
        let meta = match self.pane_metas.get(pane_id) {
            Some(m) => m,
            None => return (None, 0.0),
        };

        let mut best_provider: Option<Provider> = None;
        let mut best_confidence: f64 = 0.0;

        for detector in &self.detectors {
            if let Some(confidence) = detector.detect(meta) {
                if confidence > best_confidence {
                    best_confidence = confidence;
                    best_provider = Some(detector.id());
                }
            }
        }

        if best_confidence >= PROVIDER_DETECTION_THRESHOLD {
            debug!(
                pane_id = %pane_id,
                provider = ?best_provider,
                confidence = best_confidence,
                "provider detected"
            );
            (best_provider, best_confidence)
        } else {
            (None, 0.0)
        }
    }

    /// Update the cached PaneMeta for a pane. Called when evidence arrives,
    /// since the poller already extracts metadata from the terminal backend.
    /// We infer metadata from evidence context: the provider field tells us
    /// what builder produced it, and the reason_code carries raw state info.
    pub fn update_pane_meta(&mut self, pane_id: &str, meta: PaneMeta) {
        self.pane_metas.insert(pane_id.to_string(), meta);
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
                // Initialize an empty PaneMeta; it will be populated by the
                // poller on the next poll cycle via update_pane_meta().
                self.pane_metas.entry(pane_id.clone()).or_insert_with(|| {
                    PaneMeta {
                        pane_id: pane_id.clone(),
                        agent_type: String::new(),
                        current_cmd: String::new(),
                        pane_title: String::new(),
                        session_label: String::new(),
                        raw_state: String::new(),
                        raw_reason_code: String::new(),
                        last_event_type: String::new(),
                    }
                });
                let _ = self.notify_tx.send(StateNotification::PaneAdded {
                    pane_id: pane_id.clone(),
                });
            }
            TopologyEvent::PaneRemoved { ref pane_id } => {
                info!(pane_id = %pane_id, "pane removed");
                self.evidence_windows.remove(pane_id);
                self.pane_states.remove(pane_id);
                self.pane_metas.remove(pane_id);
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
        let orch = Orchestrator::new(source_rx, notify_tx, shared_state.clone(), vec![]);
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
        let mut orch = Orchestrator::new(rx, ntx, shared.clone(), vec![]);

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
            meta: None,
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
        let mut orch = Orchestrator::new(rx, ntx, shared.clone(), vec![]);

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
            meta: None,
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

    // -----------------------------------------------------------------------
    // Evidence TTL enforcement tests
    // -----------------------------------------------------------------------

    #[test]
    fn expire_evidence_filters_expired_entries() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // Create evidence that is already expired (timestamp 200s ago, TTL 90s)
        let mut ev_expired = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        ev_expired.timestamp = Utc::now() - chrono::Duration::seconds(200);
        ev_expired.ttl = Duration::from_secs(90);

        // Create evidence that is still valid (timestamp now, TTL 90s)
        let ev_valid = make_evidence(
            Provider::Codex,
            ActivityState::Idle,
            0.8,
            0.8,
            SourceType::Poller,
            "idle",
        );

        orch.evidence_windows
            .insert("%1".to_string(), vec![ev_expired, ev_valid]);

        orch.expire_evidence("%1", Utc::now());

        let window = orch.evidence_windows.get("%1").unwrap();
        assert_eq!(window.len(), 1, "expired evidence should be removed");
        assert_eq!(
            window[0].signal,
            ActivityState::Idle,
            "only the valid evidence should remain"
        );
    }

    #[test]
    fn expired_evidence_filtered_before_evaluation() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // Create evidence that is already expired
        let mut ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        ev.timestamp = Utc::now() - chrono::Duration::seconds(200);
        ev.ttl = Duration::from_secs(90);

        // Add a valid evidence so pane state gets created
        let ev_valid = make_evidence(
            Provider::Claude,
            ActivityState::WaitingInput,
            0.95,
            0.95,
            SourceType::Hook,
            "waiting",
        );

        orch.ingest_evidence("%1", vec![ev, ev_valid]);

        // Before evaluate_pane, the window has 2 entries (different sources needed
        // for both to remain, but they share provider+source so supersede applies).
        // Let's use different sources to keep both.
        orch.evidence_windows.clear();
        let mut ev_expired = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        ev_expired.timestamp = Utc::now() - chrono::Duration::seconds(200);
        ev_expired.ttl = Duration::from_secs(90);

        let ev_valid2 = make_evidence(
            Provider::Codex,
            ActivityState::WaitingInput,
            0.95,
            0.95,
            SourceType::Api,
            "waiting",
        );
        orch.evidence_windows
            .insert("%1".to_string(), vec![ev_expired, ev_valid2]);

        orch.evaluate_pane("%1");

        // After evaluation, expired evidence should have been removed
        let window = orch.evidence_windows.get("%1").unwrap();
        assert_eq!(
            window.len(),
            1,
            "expired evidence should be removed during evaluation"
        );

        // The resolved state should be based only on the valid evidence
        let state = orch.get_pane_state("%1").unwrap();
        assert_eq!(
            state.activity.state,
            ActivityState::WaitingInput,
            "state should reflect only non-expired evidence"
        );
    }

    #[test]
    fn pane_with_only_expired_evidence_resolves_to_unknown() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // Create evidence that is already expired
        let mut ev = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        ev.timestamp = Utc::now() - chrono::Duration::seconds(200);
        ev.ttl = Duration::from_secs(90);

        orch.evidence_windows
            .insert("%1".to_string(), vec![ev]);

        orch.evaluate_pane("%1");

        // With all evidence expired, the engine receives an empty slice
        // and should resolve to Unknown
        let state = orch.get_pane_state("%1").unwrap();
        assert_eq!(
            state.activity.state,
            ActivityState::Unknown,
            "pane with only expired evidence should resolve to Unknown"
        );
        assert_eq!(state.activity.confidence, 0.0);
    }

    #[test]
    fn evidence_window_cleaned_on_pane_removed() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // Populate evidence for two panes
        let ev1 = make_evidence(
            Provider::Claude,
            ActivityState::Running,
            0.9,
            0.9,
            SourceType::Hook,
            "running",
        );
        let ev2 = make_evidence(
            Provider::Codex,
            ActivityState::Idle,
            0.8,
            0.8,
            SourceType::Poller,
            "idle",
        );
        orch.ingest_evidence("%1", vec![ev1]);
        orch.ingest_evidence("%2", vec![ev2]);
        orch.evaluate_pane("%1");
        orch.evaluate_pane("%2");

        assert!(orch.evidence_windows.contains_key("%1"));
        assert!(orch.evidence_windows.contains_key("%2"));
        assert!(orch.pane_states.contains_key("%1"));
        assert!(orch.pane_states.contains_key("%2"));

        // Remove pane %1
        orch.handle_topology(TopologyEvent::PaneRemoved {
            pane_id: "%1".into(),
        });

        // %1 should be cleaned up, %2 should remain
        assert!(
            !orch.evidence_windows.contains_key("%1"),
            "evidence window should be removed for removed pane"
        );
        assert!(
            !orch.pane_states.contains_key("%1"),
            "pane state should be removed for removed pane"
        );
        assert!(
            orch.evidence_windows.contains_key("%2"),
            "other panes should not be affected"
        );
        assert!(
            orch.pane_states.contains_key("%2"),
            "other pane states should not be affected"
        );
    }

    // -----------------------------------------------------------------------
    // Provider detection tests
    // -----------------------------------------------------------------------

    /// A test-only ProviderDetector that returns a fixed confidence for any
    /// PaneMeta whose `current_cmd` contains the given token.
    struct MockDetector {
        provider: Provider,
        cmd_token: String,
        confidence: f64,
    }

    impl MockDetector {
        fn new(provider: Provider, cmd_token: &str, confidence: f64) -> Self {
            Self {
                provider,
                cmd_token: cmd_token.to_string(),
                confidence,
            }
        }
    }

    impl ProviderDetector for MockDetector {
        fn id(&self) -> Provider {
            self.provider
        }

        fn detect(&self, meta: &PaneMeta) -> Option<f64> {
            if meta.current_cmd.contains(&self.cmd_token) {
                Some(self.confidence)
            } else {
                None
            }
        }
    }

    /// Helper to create an orchestrator with specific detectors.
    fn create_orchestrator_with_detectors(
        detectors: Vec<Box<dyn ProviderDetector>>,
    ) -> (
        mpsc::Sender<SourceEvent>,
        broadcast::Receiver<StateNotification>,
        Orchestrator,
        SharedState,
    ) {
        let (source_tx, source_rx) = mpsc::channel(64);
        let (notify_tx, notify_rx) = broadcast::channel(64);
        let shared_state: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let orch = Orchestrator::new(source_rx, notify_tx, shared_state.clone(), detectors);
        (source_tx, notify_rx, orch, shared_state)
    }

    #[test]
    fn detect_provider_returns_correct_provider_when_detector_matches() {
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.9)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        // Set up PaneMeta for the pane with current_cmd containing "claude"
        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "claude".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        let (provider, confidence) = orch.detect_provider("%1");
        assert_eq!(provider, Some(Provider::Claude));
        assert!((confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_provider_returns_none_when_no_detector_matches() {
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.9)),
            Box::new(MockDetector::new(Provider::Codex, "codex", 0.85)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        // Set up PaneMeta with a command that matches no detector
        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "bash".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        let (provider, confidence) = orch.detect_provider("%1");
        assert_eq!(provider, None);
        assert!((confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_provider_returns_none_when_no_meta_exists() {
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.9)),
        ];
        let (_tx, _rx, orch, _shared) = create_orchestrator_with_detectors(detectors);

        // No PaneMeta registered for this pane
        let (provider, confidence) = orch.detect_provider("%unknown");
        assert_eq!(provider, None);
        assert!((confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_provider_highest_confidence_wins_among_multiple_detectors() {
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "agent", 0.7)),
            Box::new(MockDetector::new(Provider::Codex, "agent", 0.95)),
            Box::new(MockDetector::new(Provider::Gemini, "agent", 0.6)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        // All three detectors match (current_cmd contains "agent"), but Codex
        // has the highest confidence.
        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "agent".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        let (provider, confidence) = orch.detect_provider("%1");
        assert_eq!(provider, Some(Provider::Codex));
        assert!((confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_provider_below_threshold_returns_none() {
        // Detector returns 0.4 which is below the 0.5 threshold
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.4)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "claude".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        let (provider, confidence) = orch.detect_provider("%1");
        assert_eq!(provider, None, "confidence 0.4 is below threshold 0.5");
        assert!((confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn detect_provider_at_threshold_returns_match() {
        // Detector returns exactly 0.5 which equals the threshold
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.5)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "claude".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        let (provider, confidence) = orch.detect_provider("%1");
        assert_eq!(
            provider,
            Some(Provider::Claude),
            "confidence exactly at threshold should match"
        );
        assert!((confidence - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_pane_populates_provider_from_detection() {
        let detectors: Vec<Box<dyn ProviderDetector>> = vec![
            Box::new(MockDetector::new(Provider::Claude, "claude", 0.86)),
        ];
        let (_tx, _rx, mut orch, _shared) = create_orchestrator_with_detectors(detectors);

        // Set up PaneMeta
        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "claude".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );

        // Ingest evidence and evaluate
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

        let state = orch.get_pane_state("%1").expect("pane state should exist");
        assert_eq!(state.provider, Some(Provider::Claude));
        assert!((state.provider_confidence - 0.86).abs() < f64::EPSILON);
    }

    #[test]
    fn pane_meta_cleaned_on_pane_removed() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        // Set up PaneMeta
        orch.update_pane_meta(
            "%1",
            PaneMeta {
                pane_id: "%1".into(),
                agent_type: String::new(),
                current_cmd: "claude".into(),
                pane_title: String::new(),
                session_label: String::new(),
                raw_state: String::new(),
                raw_reason_code: String::new(),
                last_event_type: String::new(),
            },
        );
        assert!(orch.pane_metas.contains_key("%1"));

        // Remove the pane
        orch.handle_topology(TopologyEvent::PaneRemoved {
            pane_id: "%1".into(),
        });

        assert!(
            !orch.pane_metas.contains_key("%1"),
            "pane_metas should be cleaned up on pane removal"
        );
    }

    #[test]
    fn topology_pane_added_initializes_empty_meta() {
        let (_tx, _rx, mut orch, _shared) = create_orchestrator();

        orch.handle_topology(TopologyEvent::PaneAdded {
            pane_id: "%5".into(),
        });

        assert!(
            orch.pane_metas.contains_key("%5"),
            "PaneAdded should initialize an empty PaneMeta"
        );
        let meta = &orch.pane_metas["%5"];
        assert_eq!(meta.pane_id, "%5");
        assert_eq!(meta.current_cmd, "");
        assert_eq!(meta.pane_title, "");
    }

    // -----------------------------------------------------------------------
    // CancellationToken / graceful shutdown tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn orchestrator_stops_when_token_cancelled() {
        use tokio_util::sync::CancellationToken;

        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel::<SourceEvent>(64);
        let (ntx, _nrx) = broadcast::channel::<StateNotification>(64);
        let shared: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let mut orch = Orchestrator::with_cancel(rx, ntx, shared.clone(), vec![], cancel.clone());

        // Send an event before cancellation
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
            meta: None,
        })
        .await
        .unwrap();

        // Cancel after a short delay
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        // run() should return once the token is cancelled (even though
        // the channel is still open because `tx` is alive).
        let result = tokio::time::timeout(Duration::from_secs(2), orch.run()).await;
        assert!(result.is_ok(), "orchestrator should exit within timeout after cancellation");

        // The event sent before cancellation should have been processed.
        let state = orch.get_pane_state("%1").expect("pane %1 should exist");
        assert_eq!(state.activity.state, ActivityState::Running);
    }

    #[tokio::test]
    async fn orchestrator_exits_on_channel_close_even_without_cancel() {
        // Verify backward compatibility: the orchestrator still exits when the
        // source channel is closed, even when using with_cancel and a never-
        // cancelled token.
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel::<SourceEvent>(64);
        let (ntx, _nrx) = broadcast::channel::<StateNotification>(64);
        let shared: SharedState = Arc::new(RwLock::new(DaemonState::default()));
        let mut orch = Orchestrator::with_cancel(rx, ntx, shared, vec![], cancel);

        // Drop the sender to close the channel.
        drop(tx);

        let result = tokio::time::timeout(Duration::from_secs(2), orch.run()).await;
        assert!(result.is_ok(), "orchestrator should exit when channel is closed");
    }
}
