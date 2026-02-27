//! agtmux-source-claude-jsonl: Deterministic source for Claude Code.
//! Reads Claude Code's JSONL transcript files as deterministic evidence,
//! providing deterministic detection without hooks registration.
//!
//! Architecture ref: docs/30_architecture.md C-007

pub mod discovery;
pub mod source;
pub mod translate;
pub mod watcher;

pub use agtmux_core_v5::types;
