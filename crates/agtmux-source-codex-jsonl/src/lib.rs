//! agtmux-source-codex-jsonl: Deterministic source for Codex (OpenAI).
//! Reads Codex JSONL session files from ~/.codex/sessions/**/*.jsonl
//! using semantic FSM state detection (not mtime heuristics).

pub mod discovery;
pub mod fsm;
pub mod source;
pub mod translate;
pub mod watcher;
