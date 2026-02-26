//! Error types for the tmux backend.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TmuxError {
    #[error("tmux command failed: {0}")]
    CommandFailed(String),

    #[error("failed to parse list-panes line {line_num}: {detail}")]
    ParseError { line_num: usize, detail: String },

    #[error("tmux io error: {0}")]
    Io(#[from] std::io::Error),
}
