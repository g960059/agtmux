//! agtmux-source-poller: Heuristic fallback source server.
//! Reuses v4 poller pattern matching to estimate pane activity state.
//! Always-on fallback for when deterministic sources are unavailable.
//!
//! Architecture ref: docs/30_architecture.md C-006

pub mod accuracy;
pub mod detect;
pub mod evidence;
pub mod source;

pub use agtmux_core_v5::types;
