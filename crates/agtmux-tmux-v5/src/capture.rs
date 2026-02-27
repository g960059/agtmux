//! Pane capture and process inspection.

use crate::error::TmuxError;
use crate::executor::TmuxCommandRunner;

/// Capture the last `lines` lines of terminal output from a pane.
pub fn capture_pane(
    runner: &impl TmuxCommandRunner,
    pane_id: &str,
    lines: u32,
) -> Result<Vec<String>, TmuxError> {
    let start_line = format!("-{lines}");
    let output = runner.run(&["capture-pane", "-p", "-S", &start_line, "-t", pane_id])?;
    Ok(output.lines().map(String::from).collect())
}

/// Known interactive shells — panes running these are plain terminals,
/// not agent runtimes, and must never receive a Codex thread assignment.
const SHELL_CMDS: &[&str] = &[
    "zsh", "bash", "fish", "sh", "csh", "tcsh", "ksh", "dash", "nu", "pwsh",
];

/// Extract a process hint from the pane's current command.
///
/// Returns:
/// - `Some("claude")` — Claude Code binary detected
/// - `Some("codex")`  — Codex CLI binary detected
/// - `Some("shell")`  — plain interactive shell (zsh, bash, …); never an agent
/// - `None`           — neutral runtime (node, python, …); may or may not be an agent
pub fn inspect_pane_processes(current_cmd: &str) -> Option<String> {
    let lower = current_cmd.to_ascii_lowercase();
    if lower.contains("claude") {
        Some("claude".to_string())
    } else if lower.contains("codex") {
        Some("codex".to_string())
    } else if SHELL_CMDS.iter().any(|&s| lower == s) {
        Some("shell".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_claude_cmd() {
        assert_eq!(inspect_pane_processes("claude"), Some("claude".to_string()));
        assert_eq!(
            inspect_pane_processes("claude code"),
            Some("claude".to_string())
        );
        assert_eq!(inspect_pane_processes("Claude"), Some("claude".to_string()));
    }

    #[test]
    fn inspect_codex_cmd() {
        assert_eq!(
            inspect_pane_processes("codex --model o3"),
            Some("codex".to_string())
        );
        assert_eq!(inspect_pane_processes("Codex"), Some("codex".to_string()));
    }

    #[test]
    fn inspect_shell_cmds() {
        for shell in &[
            "zsh", "bash", "fish", "sh", "csh", "tcsh", "ksh", "dash", "nu", "pwsh",
        ] {
            assert_eq!(
                inspect_pane_processes(shell),
                Some("shell".to_string()),
                "{shell} should be classified as shell"
            );
        }
    }

    #[test]
    fn inspect_neutral_runtime() {
        // node, python etc. are neutral (may host Codex/Claude as runtime)
        assert_eq!(inspect_pane_processes("node"), None);
        assert_eq!(inspect_pane_processes("python"), None);
        assert_eq!(inspect_pane_processes("vim"), None);
    }

    #[test]
    fn inspect_no_agent() {
        // retained for backward compat — shells now return Some("shell"), not None
        assert_eq!(inspect_pane_processes("vim"), None);
    }

    #[test]
    fn mock_capture_pane() {
        struct MockRunner;
        impl TmuxCommandRunner for MockRunner {
            fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
                assert!(args.contains(&"capture-pane"));
                assert!(args.contains(&"-p"));
                Ok("line 1\nline 2\nline 3\n".to_string())
            }
        }
        let lines = capture_pane(&MockRunner, "%0", 50).expect("should capture");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line 1");
    }

    #[test]
    fn capture_empty_pane() {
        struct MockRunner;
        impl TmuxCommandRunner for MockRunner {
            fn run(&self, _args: &[&str]) -> Result<String, TmuxError> {
                Ok(String::new())
            }
        }
        let lines = capture_pane(&MockRunner, "%0", 50).expect("should capture");
        assert!(lines.is_empty());
    }
}
