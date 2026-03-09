//! TmuxCommandRunner trait and TmuxExecutor (sync subprocess wrapper).
//! Ported from v4 pattern for mock-injectable testing.

use crate::error::TmuxError;
use std::collections::HashMap;
use std::path::Path;

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

    #[must_use]
    pub fn tmux_bin_path(&self) -> &str {
        &self.tmux_bin
    }

    #[must_use]
    pub fn target_description(&self) -> String {
        if let Some(ref path) = self.socket_path {
            format!("-S {path}")
        } else if let Some(ref name) = self.socket_name {
            format!("-L {name}")
        } else {
            "default".to_string()
        }
    }
}

impl Default for TmuxExecutor {
    fn default() -> Self {
        Self::new(resolve_tmux_bin())
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

fn resolve_tmux_bin() -> String {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    resolve_tmux_bin_from_env_with(&env, is_executable_path)
}

fn resolve_tmux_bin_from_env_with<F>(env: &HashMap<String, String>, is_executable: F) -> String
where
    F: Fn(&str) -> bool,
{
    if let Some(explicit) = env
        .get("TMUX_BIN")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if is_executable(explicit) {
            return explicit.to_string();
        }
    }

    if let Some(path) = env
        .get("PATH")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        for dir in path.split(':').filter(|segment| !segment.is_empty()) {
            let candidate = format!("{dir}/tmux");
            if is_executable(&candidate) {
                return candidate;
            }
        }
    }

    for candidate in [
        "/opt/homebrew/bin/tmux",
        "/usr/local/bin/tmux",
        "/usr/bin/tmux",
        "/bin/tmux",
    ] {
        if is_executable(candidate) {
            return candidate.to_string();
        }
    }

    "tmux".to_string()
}

fn is_executable_path(path: &str) -> bool {
    let path = Path::new(path);
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            return metadata.permissions().mode() & 0o111 != 0;
        }
        false
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn default_executor() {
        let exec = TmuxExecutor::default();
        assert!(!exec.tmux_bin.is_empty());
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

    #[test]
    fn resolve_tmux_bin_prefers_explicit_tmux_bin() {
        let env = HashMap::from([
            ("TMUX_BIN".to_string(), "/custom/tmux".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ]);

        let resolved =
            resolve_tmux_bin_from_env_with(&env, |candidate| candidate == "/custom/tmux");
        assert_eq!(resolved, "/custom/tmux");
    }

    #[test]
    fn resolve_tmux_bin_falls_back_to_path_lookup() {
        let env = HashMap::from([(
            "PATH".to_string(),
            "/usr/bin:/opt/homebrew/bin:/bin".to_string(),
        )]);

        let resolved =
            resolve_tmux_bin_from_env_with(&env, |candidate| candidate == "/opt/homebrew/bin/tmux");
        assert_eq!(resolved, "/opt/homebrew/bin/tmux");
    }

    #[test]
    fn resolve_tmux_bin_falls_back_to_standard_locations_when_path_is_stripped() {
        let env = HashMap::from([(
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        )]);

        let resolved =
            resolve_tmux_bin_from_env_with(&env, |candidate| candidate == "/opt/homebrew/bin/tmux");
        assert_eq!(resolved, "/opt/homebrew/bin/tmux");
    }

    #[test]
    fn resolve_tmux_bin_returns_tmux_when_no_candidate_is_executable() {
        let env = HashMap::new();
        let resolved = resolve_tmux_bin_from_env_with(&env, |_| false);
        assert_eq!(resolved, "tmux");
    }
}
