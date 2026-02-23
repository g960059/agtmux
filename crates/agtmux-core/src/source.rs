use crate::types::{Evidence, SourceType};
use chrono::{DateTime, Utc};

/// Marker trait for state sources.
/// Actual async implementations live in agtmux-daemon.
pub trait StateSource: Send + 'static {
    fn source_type(&self) -> SourceType;
}

/// Event emitted by sources into the orchestrator pipeline.
#[derive(Debug, Clone)]
pub enum SourceEvent {
    /// Raw signal needing normalization (from hooks, API, etc.)
    RawSignal {
        pane_id: String,
        event_type: String,
        source: SourceType,
        payload: String,
        timestamp: DateTime<Utc>,
    },
    /// Pre-built evidence (from pollers that do their own matching)
    Evidence {
        pane_id: String,
        evidence: Vec<Evidence>,
    },
    /// Topology change from the backend
    TopologyChange(TopologyEvent),
}

#[derive(Debug, Clone)]
pub enum TopologyEvent {
    PaneAdded { pane_id: String },
    PaneRemoved { pane_id: String },
}
