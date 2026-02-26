//! agtmux-tmux-v5: tmux backend IO boundary.
//! Provides subprocess execution, pane listing/capture, process inspection,
//! and pane generation tracking. No business logic — pure IO boundary.
//!
//! Architecture ref: docs/30_architecture.md C-015

pub mod capture;
pub mod error;
pub mod executor;
pub mod generation;
pub mod pane_info;
pub mod snapshot;

pub use capture::{capture_pane, inspect_pane_processes};
pub use error::TmuxError;
pub use executor::{TmuxCommandRunner, TmuxExecutor};
pub use generation::PaneGenerationTracker;
pub use pane_info::{LIST_PANES_FORMAT, TmuxPaneInfo, list_panes, parse_list_panes_output};
pub use snapshot::to_pane_snapshot;
