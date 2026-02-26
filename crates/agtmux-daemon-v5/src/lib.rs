//! agtmux-daemon-v5: Resolver, read-model projection, and client API.
//! Pulls aggregated events from the gateway, resolves tier winners,
//! projects pane/session state, and pushes updates to clients.
//!
//! Architecture ref: docs/30_architecture.md C-002

pub mod alert_routing;
pub mod binding_projection;
pub mod projection;
pub mod snapshot;
pub mod supervisor;

pub use agtmux_core_v5::types;
