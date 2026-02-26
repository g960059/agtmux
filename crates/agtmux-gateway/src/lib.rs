//! agtmux-gateway: Pull-aggregates events from multiple source servers,
//! manages per-source cursors, and serves aggregated events to the daemon.
//!
//! Architecture ref: docs/30_architecture.md C-003

pub mod cursor_hardening;
pub mod gateway;
pub mod latency_window;
pub mod source_registry;
pub mod trust_guard;

pub use agtmux_core_v5::types;
