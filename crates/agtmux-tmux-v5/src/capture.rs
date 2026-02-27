//! Pane capture and process inspection.

use std::collections::HashMap;

use crate::error::TmuxError;
use crate::executor::TmuxCommandRunner;

// ─── Process-tree types (T-128) ──────────────────────────────────────────────

/// One entry from `ps -eo pid=,ppid=,args=`.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub args: String,
}

/// Snapshot of all running processes on the host, keyed by PID.
/// Built once per poll tick via `scan_all_processes`.
pub type ProcessMap = HashMap<u32, ProcessInfo>;

/// Scan all running processes using `ps -eo pid=,ppid=,args=`.
///
/// Called once per tick; returns an empty map on failure (non-fatal).
pub fn scan_all_processes() -> ProcessMap {
    let output = match std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,args="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return ProcessMap::new(),
    };
    match String::from_utf8(output.stdout) {
        Ok(s) => parse_ps_output(&s),
        Err(_) => ProcessMap::new(),
    }
}

fn parse_ps_output(output: &str) -> ProcessMap {
    let mut map = ProcessMap::new();
    for line in output.lines() {
        if let Some(info) = parse_ps_line(line) {
            map.insert(info.pid, info);
        }
    }
    map
}

fn parse_ps_line(line: &str) -> Option<ProcessInfo> {
    let s = line.trim();
    if s.is_empty() {
        return None;
    }
    // PID — first whitespace-delimited token
    let ws = s.find(|c: char| c.is_ascii_whitespace())?;
    let pid: u32 = s[..ws].parse().ok()?;
    let s = s[ws..].trim_start();
    // PPID — second token
    let ws = s.find(|c: char| c.is_ascii_whitespace()).unwrap_or(s.len());
    let ppid: u32 = s[..ws].parse().ok()?;
    let args = if ws < s.len() {
        s[ws..].trim_start().to_string()
    } else {
        String::new()
    };
    Some(ProcessInfo { pid, ppid, args })
}

/// Deep process-tree inspection: examines `pane_pid` and its direct children
/// to distinguish agents that share the same parent runtime (e.g. `node`).
///
/// Returns:
/// - `Some("claude")`          — a process argv identifies Claude Code
/// - `Some("codex")`           — a process argv identifies Codex CLI
/// - `Some("runtime_unknown")` — neutral runtime with unidentifiable children (fail-closed)
/// - Falls through to `inspect_pane_processes(current_cmd)` when `pane_pid` has no
///   child processes or the command is directly identifiable (shell / explicit agent)
pub fn inspect_pane_processes_deep(
    current_cmd: &str,
    pane_pid: u32,
    process_map: &ProcessMap,
) -> Option<String> {
    // Fast path: directly identifiable commands skip the process-tree scan.
    let shallow = inspect_pane_processes(current_cmd);
    match shallow.as_deref() {
        Some("shell") | Some("codex") | Some("claude") => return shallow,
        _ => {}
    }

    // Collect pane_pid itself and its direct children.
    let pids_to_check: Vec<u32> = std::iter::once(pane_pid)
        .chain(
            process_map
                .values()
                .filter(|p| p.ppid == pane_pid)
                .map(|p| p.pid),
        )
        .collect();

    let mut found_neutral_child = false;
    for pid in pids_to_check {
        if let Some(info) = process_map.get(&pid) {
            if is_claude_argv(&info.args) {
                return Some("claude".to_string());
            }
            if is_codex_argv(&info.args) {
                return Some("codex".to_string());
            }
            if pid != pane_pid {
                found_neutral_child = true;
            }
        }
    }

    // Neutral runtime (node/bun/…) with children that couldn't be identified → fail-closed.
    if shallow.is_none() && found_neutral_child {
        return Some("runtime_unknown".to_string());
    }

    // No children (or pane_pid not in map) → fall back to shallow inspection.
    shallow
}

fn is_claude_argv(args: &str) -> bool {
    let lower = args.to_ascii_lowercase();
    lower.contains("claude")
        && !lower.contains("claude_desktop")
        && !lower.contains("claude-desktop")
}

fn is_codex_argv(args: &str) -> bool {
    args.to_ascii_lowercase().contains("codex")
}

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

    // ─── parse_ps_output tests ────────────────────────────────────────

    #[test]
    fn parse_ps_output_basic() {
        let output = "    1     0 /sbin/launchd\n12345  6789 node /path/to/claude/cli.js\n";
        let map = parse_ps_output(output);
        assert_eq!(map.len(), 2);
        let p1 = &map[&1];
        assert_eq!(p1.ppid, 0);
        assert_eq!(p1.args, "/sbin/launchd");
        let p2 = &map[&12345];
        assert_eq!(p2.ppid, 6789);
        assert_eq!(p2.args, "node /path/to/claude/cli.js");
    }

    #[test]
    fn parse_ps_output_empty_lines_skipped() {
        let output = "\n   \n42 1 sleep 60\n";
        let map = parse_ps_output(output);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&42));
    }

    #[test]
    fn parse_ps_output_no_args() {
        let output = "100 50\n"; // no args column
        let map = parse_ps_output(output);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&100].args, "");
    }

    // ─── inspect_pane_processes_deep tests ───────────────────────────

    fn make_pm(entries: &[(u32, u32, &str)]) -> ProcessMap {
        entries
            .iter()
            .map(|&(pid, ppid, args)| {
                (
                    pid,
                    ProcessInfo {
                        pid,
                        ppid,
                        args: args.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn deep_inspect_claude_child() {
        let pm = make_pm(&[
            (10, 1, "zsh"),
            (11, 10, "node /Users/user/.local/share/claude/cli.js"),
        ]);
        assert_eq!(
            inspect_pane_processes_deep("node", 10, &pm),
            Some("claude".to_string())
        );
    }

    #[test]
    fn deep_inspect_codex_child() {
        let pm = make_pm(&[
            (20, 1, "zsh"),
            (
                21,
                20,
                "node /usr/local/lib/node_modules/@openai/codex/dist/cli.mjs exec",
            ),
        ]);
        assert_eq!(
            inspect_pane_processes_deep("node", 20, &pm),
            Some("codex".to_string())
        );
    }

    #[test]
    fn deep_inspect_runtime_unknown_when_child_unidentifiable() {
        let pm = make_pm(&[
            (30, 1, "zsh"),
            (31, 30, "node /path/to/some-other-tool/index.js"),
        ]);
        assert_eq!(
            inspect_pane_processes_deep("node", 30, &pm),
            Some("runtime_unknown".to_string())
        );
    }

    #[test]
    fn deep_inspect_no_children_falls_back_to_shallow() {
        // pane_pid itself is node but no children → no runtime_unknown, fall back to None
        let pm = make_pm(&[(40, 1, "node")]);
        assert_eq!(inspect_pane_processes_deep("node", 40, &pm), None);
    }

    #[test]
    fn deep_inspect_shell_fast_path() {
        // Shell is identified by shallow inspection; process tree not needed
        let pm = make_pm(&[(50, 1, "zsh"), (51, 50, "something")]);
        assert_eq!(
            inspect_pane_processes_deep("zsh", 50, &pm),
            Some("shell".to_string())
        );
    }

    #[test]
    fn deep_inspect_explicit_codex_cmd_fast_path() {
        // "codex" in current_cmd → fast path without tree scan
        let pm = make_pm(&[]);
        assert_eq!(
            inspect_pane_processes_deep("codex", 99, &pm),
            Some("codex".to_string())
        );
    }

    #[test]
    fn deep_inspect_excludes_claude_desktop() {
        // "claude_desktop" in argv should NOT be classified as claude
        let pm = make_pm(&[
            (60, 1, "zsh"),
            (61, 60, "node /Applications/claude_desktop/app.js"),
        ]);
        // child has "claude" but also "claude_desktop" → excluded; no other match → runtime_unknown
        assert_eq!(
            inspect_pane_processes_deep("node", 60, &pm),
            Some("runtime_unknown".to_string())
        );
    }

    // ─── inspect_pane_processes tests ────────────────────────────────

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
