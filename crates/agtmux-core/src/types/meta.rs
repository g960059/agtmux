use serde::{Deserialize, Serialize};

/// Backend-agnostic pane metadata for provider detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneMeta {
    pub pane_id: String,
    pub agent_type: String,
    pub current_cmd: String,
    pub pane_title: String,
    pub session_label: String,
    pub raw_state: String,
    pub raw_reason_code: String,
    pub last_event_type: String,
}

/// Raw pane info returned by TerminalBackend::list_panes().
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawPane {
    pub pane_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_name: String,
    pub pane_title: String,
    pub current_cmd: String,
    pub width: u16,
    pub height: u16,
    pub is_active: bool,
}
