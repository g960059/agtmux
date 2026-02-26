//! TmuxCommandRunner trait and TmuxExecutor (sync subprocess wrapper).
//! Ported from v4 pattern for mock-injectable testing.

use crate::error::TmuxError;

/// Trait for executing tmux commands. Enables mock injection for testing.
pub trait TmuxCommandRunner: Send + Sync {
    fn run(&self, args: &[&str]) -> Result<String, TmuxError>;
}

impl<T: TmuxCommandRunner + ?Sized> TmuxCommandRunner for &T {
    fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
        (**self).run(args)
    }
}

/// Real tmux executor using `std::process::Command`.
pub struct TmuxExecutor {
    tmux_bin: String,
    socket_path: Option<String>,
    socket_name: Option<String>,
}

impl TmuxExecutor {
    pub fn new(tmux_bin: impl Into<String>) -> Self {
        Self {
            tmux_bin: tmux_bin.into(),
            socket_path: None,
            socket_name: None,
        }
    }

    #[must_use]
    pub fn with_socket_path(mut self, path: impl Into<String>) -> Self {
        self.socket_path = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_socket_name(mut self, name: impl Into<String>) -> Self {
        self.socket_name = Some(name.into());
        self
    }
}

impl Default for TmuxExecutor {
    fn default() -> Self {
        Self::new("tmux")
    }
}

impl TmuxCommandRunner for TmuxExecutor {
    fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
        let mut cmd = std::process::Command::new(&self.tmux_bin);
        // Socket path takes precedence over socket name
        if let Some(ref path) = self.socket_path {
            cmd.args(["-S", path]);
        } else if let Some(ref name) = self.socket_name {
            cmd.args(["-L", name]);
        }
        cmd.args(args);
        let output = cmd.output().map_err(TmuxError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TmuxError::CommandFailed(format!(
                "exit code {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_executor() {
        let exec = TmuxExecutor::default();
        assert_eq!(exec.tmux_bin, "tmux");
        assert!(exec.socket_path.is_none());
        assert!(exec.socket_name.is_none());
    }

    #[test]
    fn with_socket_path() {
        let exec = TmuxExecutor::default().with_socket_path("/tmp/my.sock");
        assert_eq!(exec.socket_path, Some("/tmp/my.sock".to_string()));
    }

    #[test]
    fn with_socket_name() {
        let exec = TmuxExecutor::default().with_socket_name("myname");
        assert_eq!(exec.socket_name, Some("myname".to_string()));
    }

    #[test]
    fn blanket_ref_impl() {
        struct Mock;
        impl TmuxCommandRunner for Mock {
            fn run(&self, _args: &[&str]) -> Result<String, TmuxError> {
                Ok("ok".to_string())
            }
        }
        let mock = Mock;
        let r: &Mock = &mock;
        assert_eq!(r.run(&[]).expect("ok"), "ok");
    }
}
