use std::process::{Command, Output};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TmuxError {
    #[error("tmux command failed: {0}")]
    CommandFailed(String),
    #[error("tmux not found")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
}

/// Synchronous tmux command executor.
///
/// Each call spawns a new `tmux` process, so the executor itself is
/// `Send + Sync` (no interior mutability, no persistent child handle).
pub struct TmuxExecutor {
    tmux_bin: String,
}

impl TmuxExecutor {
    /// Create an executor using the default `tmux` binary on `$PATH`.
    pub fn new() -> Self {
        Self {
            tmux_bin: "tmux".into(),
        }
    }

    /// Create an executor using a custom tmux binary path.
    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self {
            tmux_bin: bin.into(),
        }
    }

    /// Run a tmux command and return stdout as a `String`.
    ///
    /// Returns `TmuxError::CommandFailed` if the process exits with a
    /// non-zero status.  Returns `TmuxError::NotFound` if the binary
    /// cannot be found (mapped from `io::ErrorKind::NotFound`).
    pub fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
        let output = self.run_raw(args)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TmuxError::CommandFailed(format!(
                "exit {}: {}",
                output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                stderr.trim(),
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a tmux command and return the raw `Output` without checking
    /// the exit status.
    pub fn run_unchecked(&self, args: &[&str]) -> Result<Output, TmuxError> {
        self.run_raw(args)
    }

    // ------------------------------------------------------------------
    // internal
    // ------------------------------------------------------------------

    fn run_raw(&self, args: &[&str]) -> Result<Output, TmuxError> {
        Command::new(&self.tmux_bin)
            .args(args)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    TmuxError::NotFound
                } else {
                    TmuxError::Io(e)
                }
            })
    }
}

impl Default for TmuxExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_binary() {
        let exec = TmuxExecutor::with_bin("/nonexistent/tmux-binary");
        let err = exec.run(&["list-sessions"]).unwrap_err();
        assert!(
            matches!(err, TmuxError::NotFound),
            "expected NotFound, got: {err:?}"
        );
    }

    #[test]
    fn run_unchecked_returns_output() {
        // Even if tmux isn't running, run_unchecked should give us an
        // Output rather than an Err (assuming `tmux` binary exists).
        // If tmux isn't installed in CI this test is effectively skipped
        // via the NotFound guard.
        let exec = TmuxExecutor::new();
        match exec.run_unchecked(&["list-sessions"]) {
            Ok(output) => {
                // We got an Output — success or failure code, both fine.
                let _ = output.status;
            }
            Err(TmuxError::NotFound) => {
                // tmux not installed — acceptable in CI.
            }
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }
}
