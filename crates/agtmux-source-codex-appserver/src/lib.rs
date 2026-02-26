//! agtmux-source-codex-appserver: Deterministic source server for Codex.
//! Normalizes Codex app-server lifecycle events into SourceEventV2.
//!
//! Architecture ref: docs/30_architecture.md C-004

pub mod source;
pub mod translate;

pub use agtmux_core_v5::types;
