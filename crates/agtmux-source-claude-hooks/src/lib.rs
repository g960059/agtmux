//! agtmux-source-claude-hooks: Deterministic source server for Claude.
//! Normalizes Claude hook events into SourceEventV2.
//!
//! Architecture ref: docs/30_architecture.md C-005

pub mod source;
pub mod translate;

pub use agtmux_core_v5::types;
