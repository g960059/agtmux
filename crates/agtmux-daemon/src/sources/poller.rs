use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use agtmux_core::adapt::EvidenceBuilder;
use agtmux_core::backend::TerminalBackend;
use agtmux_core::source::{SourceEvent, TopologyEvent};
use agtmux_core::types::{PaneMeta, RawPane};
use agtmux_tmux::TmuxBackend;
use chrono::Utc;
use tokio::sync::mpsc;

/// Periodically captures tmux pane output and runs TOML-loaded
/// `EvidenceBuilder`s against each pane to produce `SourceEvent`s.
pub struct PollerSource {
    backend: Arc<TmuxBackend>,
    builders: Vec<Box<dyn EvidenceBuilder>>,
    tx: mpsc::Sender<SourceEvent>,
    interval: Duration,
    /// Tracks known pane IDs for topology-change detection.
    known_panes: HashSet<String>,
}

impl PollerSource {
    pub fn new(
        backend: Arc<TmuxBackend>,
        builders: Vec<Box<dyn EvidenceBuilder>>,
        tx: mpsc::Sender<SourceEvent>,
        interval: Duration,
    ) -> Self {
        Self {
            backend,
            builders,
            tx,
            interval,
            known_panes: HashSet::new(),
        }
    }

    /// Run the polling loop. Blocks until cancelled.
    pub async fn run(&mut self) {
        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;
            if let Err(e) = self.poll_once().await {
                tracing::warn!("poller error: {e}");
            }
        }
    }

    async fn poll_once(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // 1. list_panes via backend (sync call on blocking thread).
        let panes: Vec<RawPane> = {
            let backend = Arc::clone(&self.backend);
            tokio::task::spawn_blocking(move || backend.list_panes()).await??
        };

        // 2. Detect topology changes (added/removed panes).
        let current_ids: HashSet<String> = panes.iter().map(|p| p.pane_id.clone()).collect();

        // Detect newly added panes.
        for id in &current_ids {
            if !self.known_panes.contains(id) {
                let event = SourceEvent::TopologyChange(TopologyEvent::PaneAdded {
                    pane_id: id.clone(),
                });
                if let Err(e) = self.tx.send(event).await {
                    tracing::warn!(pane_id = %id, "failed to send PaneAdded event: {e}");
                }
            }
        }

        // Detect removed panes.
        for id in self.known_panes.iter() {
            if !current_ids.contains(id) {
                let event = SourceEvent::TopologyChange(TopologyEvent::PaneRemoved {
                    pane_id: id.clone(),
                });
                if let Err(e) = self.tx.send(event).await {
                    tracing::warn!(pane_id = %id, "failed to send PaneRemoved event: {e}");
                }
            }
        }

        self.known_panes = current_ids;

        // 3. For each pane, capture output and run evidence builders.
        let now = Utc::now();
        for raw in &panes {
            let pane_id = raw.pane_id.clone();

            // capture_pane is a sync call — run on blocking thread.
            let capture_result = {
                let backend = Arc::clone(&self.backend);
                let pid = pane_id.clone();
                tokio::task::spawn_blocking(move || backend.capture_pane(&pid)).await?
            };

            let capture_output = match capture_result {
                Ok(output) => output,
                Err(e) => {
                    tracing::warn!(pane_id = %pane_id, "capture_pane failed: {e}");
                    continue;
                }
            };

            let meta = raw_pane_to_meta(raw, &capture_output);

            // Run each evidence builder against this pane's meta.
            let mut all_evidence = Vec::new();
            for builder in &self.builders {
                let evidence = builder.build_evidence(&meta, now);
                all_evidence.extend(evidence);
            }

            if !all_evidence.is_empty() {
                let event = SourceEvent::Evidence {
                    pane_id: pane_id.clone(),
                    evidence: all_evidence,
                    meta: Some(meta),
                };
                if let Err(e) = self.tx.send(event).await {
                    tracing::warn!(pane_id = %pane_id, "failed to send Evidence event: {e}");
                }
            }
        }

        Ok(())
    }
}

/// Convert a `RawPane` plus captured terminal output into `PaneMeta`
/// suitable for `EvidenceBuilder` pattern matching.
fn raw_pane_to_meta(raw: &RawPane, capture_output: &str) -> PaneMeta {
    PaneMeta {
        pane_id: raw.pane_id.clone(),
        agent_type: String::new(), // set by provider detection later
        current_cmd: raw.current_cmd.clone(),
        pane_title: raw.pane_title.clone(),
        session_label: raw.session_name.clone(),
        raw_state: capture_output.to_string(),
        raw_reason_code: String::new(),
        last_event_type: String::new(),
    }
}
