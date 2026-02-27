//! TmuxPaneInfo, list_panes format string, and parser.

use crate::error::TmuxError;
use crate::executor::TmuxCommandRunner;
use serde::{Deserialize, Serialize};

/// Tab-delimited format string for `tmux list-panes -a -F`.
pub const LIST_PANES_FORMAT: &str = "#{session_id}\t#{session_name}\t#{window_id}\t#{window_name}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{pane_title}\t#{pane_width}\t#{pane_height}\t#{pane_active}\t#{session_attached}\t#{pane_pid}";

/// Full metadata for a tmux pane.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TmuxPaneInfo {
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_name: String,
    pub pane_id: String,
    pub current_cmd: String,
    pub current_path: String,
    pub pane_title: String,
    pub width: u16,
    pub height: u16,
    pub active: bool,
    pub session_attached: bool,
    /// PID of the process running in this pane (tmux `#{pane_pid}`).
    /// Used for deep process-tree inspection (T-128).
    pub pane_pid: Option<u32>,
}

/// Execute `tmux list-panes -a` and parse the output.
pub fn list_panes(runner: &impl TmuxCommandRunner) -> Result<Vec<TmuxPaneInfo>, TmuxError> {
    let output = runner.run(&["list-panes", "-a", "-F", LIST_PANES_FORMAT])?;
    parse_list_panes_output(&output)
}

/// Parse the raw output of `tmux list-panes -a -F <FORMAT>`.
pub fn parse_list_panes_output(output: &str) -> Result<Vec<TmuxPaneInfo>, TmuxError> {
    let mut panes = Vec::new();
    for (idx, line) in output.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pane = parse_line(trimmed, idx + 1)?;
        panes.push(pane);
    }
    Ok(panes)
}

fn parse_line(line: &str, line_num: usize) -> Result<TmuxPaneInfo, TmuxError> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 11 {
        return Err(TmuxError::ParseError {
            line_num,
            detail: format!(
                "expected at least 11 tab-separated fields, got {}",
                parts.len()
            ),
        });
    }

    let width = parts[8].parse::<u16>().unwrap_or(80);
    let height = parts[9].parse::<u16>().unwrap_or(24);
    let active = parse_bool(parts[10]);
    let session_attached = if parts.len() > 11 {
        parse_bool(parts[11])
    } else {
        false
    };
    let pane_pid: Option<u32> = parts.get(12).and_then(|s| s.trim().parse().ok());

    Ok(TmuxPaneInfo {
        session_id: parts[0].to_string(),
        session_name: parts[1].to_string(),
        window_id: parts[2].to_string(),
        window_name: parts[3].to_string(),
        pane_id: parts[4].to_string(),
        current_cmd: parts[5].to_string(),
        current_path: parts[6].to_string(),
        pane_title: parts[7].to_string(),
        width,
        height,
        active,
        session_attached,
        pane_pid,
    })
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "1" | "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_line() {
        let line = "$0\tmain\t@0\tdev\t%0\tzsh\t/home/user\tpane-title\t200\t50\t1\t1";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.session_id, "$0");
        assert_eq!(pane.session_name, "main");
        assert_eq!(pane.window_id, "@0");
        assert_eq!(pane.window_name, "dev");
        assert_eq!(pane.pane_id, "%0");
        assert_eq!(pane.current_cmd, "zsh");
        assert_eq!(pane.current_path, "/home/user");
        assert_eq!(pane.pane_title, "pane-title");
        assert_eq!(pane.width, 200);
        assert_eq!(pane.height, 50);
        assert!(pane.active);
        assert!(pane.session_attached);
    }

    #[test]
    fn parse_inactive_detached() {
        let line = "$1\twork\t@1\teditor\t%1\tvim\t/tmp\ttitle\t80\t24\t0\t0";
        let pane = parse_line(line, 1).expect("should parse");
        assert!(!pane.active);
        assert!(!pane.session_attached);
    }

    #[test]
    fn parse_multiple_panes() {
        let output = [
            "$0\tmain\t@0\tdev\t%0\tzsh\t/home\ttitle0\t200\t50\t1\t1",
            "$0\tmain\t@0\tdev\t%1\tclaude\t/home\tclaude code\t200\t50\t0\t1",
        ]
        .join("\n");
        let panes = parse_list_panes_output(&output).expect("should parse");
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].pane_id, "%0");
        assert_eq!(panes[1].pane_id, "%1");
        assert_eq!(panes[1].current_cmd, "claude");
    }

    #[test]
    fn parse_empty_output() {
        let panes = parse_list_panes_output("").expect("should parse");
        assert!(panes.is_empty());
    }

    #[test]
    fn parse_legacy_11_fields() {
        let line = "$0\tmain\t@0\tdev\t%0\tzsh\t/home\ttitle\t80\t24\t1";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.pane_id, "%0");
        assert!(!pane.session_attached);
    }

    #[test]
    fn parse_too_few_fields_error() {
        let result = parse_line("$0\tmain\t@0", 1);
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_width_defaults() {
        let line = "$0\tmain\t@0\tdev\t%0\tzsh\t/home\ttitle\tXX\tYY\t1\t1";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.width, 80);
        assert_eq!(pane.height, 24);
    }

    #[test]
    fn mock_runner_list_panes() {
        struct MockRunner;
        impl TmuxCommandRunner for MockRunner {
            fn run(&self, args: &[&str]) -> Result<String, TmuxError> {
                assert!(args.contains(&"list-panes"));
                Ok(
                    "$0\tmain\t@0\tdev\t%0\tclaude\t/home\tclaude code\t200\t50\t1\t1\n"
                        .to_string(),
                )
            }
        }
        let panes = list_panes(&MockRunner).expect("should list");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].current_cmd, "claude");
    }

    #[test]
    fn parse_bool_variants() {
        assert!(parse_bool("1"));
        assert!(parse_bool("true"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("false"));
        assert!(!parse_bool(""));
    }

    #[test]
    fn pane_title_with_spaces() {
        let line = "$0\tmain\t@0\tdev\t%0\tclaude\t/home\tmy cool pane title\t80\t24\t1\t1";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.pane_title, "my cool pane title");
    }

    #[test]
    fn parse_with_pane_pid() {
        let line = "$0\tmain\t@0\tdev\t%0\tnode\t/home\ttitle\t80\t24\t1\t1\t12345";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.pane_pid, Some(12345));
    }

    #[test]
    fn parse_without_pane_pid_defaults_to_none() {
        // 12-field format (legacy, no pane_pid column) → pane_pid = None
        let line = "$0\tmain\t@0\tdev\t%0\tnode\t/home\ttitle\t80\t24\t1\t1";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.pane_pid, None);
    }

    #[test]
    fn parse_pane_pid_invalid_value_defaults_to_none() {
        // Non-numeric pane_pid (e.g., empty string from tmux formatting edge case)
        let line = "$0\tmain\t@0\tdev\t%0\tnode\t/home\ttitle\t80\t24\t1\t1\t";
        let pane = parse_line(line, 1).expect("should parse");
        assert_eq!(pane.pane_pid, None);
    }
}
