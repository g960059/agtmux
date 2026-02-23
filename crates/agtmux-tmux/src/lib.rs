pub mod control_mode;
pub mod executor;
pub mod observer;
pub mod pipe_pane;

use agtmux_core::backend::TerminalBackend;
use agtmux_core::types::RawPane;
use executor::{TmuxError, TmuxExecutor};

/// tmux list-panes format string.
///
/// Fields are separated by `\t` (tab) in the following order:
///   session_name, window_id, window_name, pane_id,
///   current_command, pane_title, pane_width, pane_height, pane_active
///
/// Tab is chosen over `:` because pane titles may contain colons.
const LIST_PANES_FMT: &str = concat!(
    "#{session_name}\t",
    "#{window_id}\t",
    "#{window_name}\t",
    "#{pane_id}\t",
    "#{pane_current_command}\t",
    "#{pane_title}\t",
    "#{pane_width}\t",
    "#{pane_height}\t",
    "#{pane_active}",
);

/// Number of tab-separated fields produced by `LIST_PANES_FMT`.
const EXPECTED_FIELDS: usize = 9;

/// TmuxBackend: implements `TerminalBackend` for tmux via subprocess calls.
pub struct TmuxBackend {
    executor: TmuxExecutor,
}

impl TmuxBackend {
    /// Create a backend using the default `tmux` binary on `$PATH`.
    pub fn new() -> Self {
        Self {
            executor: TmuxExecutor::new(),
        }
    }

    /// Create a backend with a custom `TmuxExecutor`.
    pub fn with_executor(executor: TmuxExecutor) -> Self {
        Self { executor }
    }
}

impl Default for TmuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalBackend for TmuxBackend {
    type Error = TmuxError;

    fn list_panes(&self) -> Result<Vec<RawPane>, Self::Error> {
        let stdout = self
            .executor
            .run(&["list-panes", "-a", "-F", LIST_PANES_FMT])?;

        parse_list_panes_output(&stdout)
    }

    fn capture_pane(&self, pane_id: &str) -> Result<String, Self::Error> {
        self.executor.run(&["capture-pane", "-t", pane_id, "-p"])
    }

    fn select_pane(&self, pane_id: &str) -> Result<(), Self::Error> {
        self.executor.run(&["select-pane", "-t", pane_id])?;
        Ok(())
    }
}

// ------------------------------------------------------------------
// Parsing helpers
// ------------------------------------------------------------------

/// Parse the raw output of `tmux list-panes -a -F <fmt>` into a
/// `Vec<RawPane>`.
///
/// Lines that are empty or cannot be parsed are silently skipped so that
/// a single malformed line does not prevent the rest from being returned.
fn parse_list_panes_output(output: &str) -> Result<Vec<RawPane>, TmuxError> {
    let mut panes = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match parse_pane_line(line) {
            Ok(pane) => panes.push(pane),
            Err(e) => {
                tracing::warn!(%e, line, "skipping malformed list-panes line");
            }
        }
    }

    Ok(panes)
}

/// Parse a single tab-separated line into a `RawPane`.
fn parse_pane_line(line: &str) -> Result<RawPane, TmuxError> {
    // The format has exactly 9 fields separated by '\t'.
    // Tab is safe because tmux never emits literal tabs inside
    // session_name, window_name, pane_title, etc.  We use `splitn`
    // with EXPECTED_FIELDS so that a tab inside the last field is
    // preserved.
    let parts: Vec<&str> = line.splitn(EXPECTED_FIELDS, '\t').collect();

    if parts.len() < EXPECTED_FIELDS {
        return Err(TmuxError::Parse(format!(
            "expected {EXPECTED_FIELDS} fields, got {}: {line}",
            parts.len()
        )));
    }

    let width: u16 = parts[6].parse().map_err(|_| {
        TmuxError::Parse(format!("invalid width '{}' in line: {line}", parts[6]))
    })?;

    let height: u16 = parts[7].parse().map_err(|_| {
        TmuxError::Parse(format!("invalid height '{}' in line: {line}", parts[7]))
    })?;

    let is_active = parts[8] == "1";

    Ok(RawPane {
        session_name: parts[0].to_string(),
        window_id: parts[1].to_string(),
        window_name: parts[2].to_string(),
        pane_id: parts[3].to_string(),
        current_cmd: parts[4].to_string(),
        pane_title: parts[5].to_string(),
        width,
        height,
        is_active,
    })
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_line() {
        let line = "main\t@0\teditor\t%1\tvim\t~/code\t120\t40\t1";
        let pane = parse_pane_line(line).unwrap();
        assert_eq!(pane.session_name, "main");
        assert_eq!(pane.window_id, "@0");
        assert_eq!(pane.window_name, "editor");
        assert_eq!(pane.pane_id, "%1");
        assert_eq!(pane.current_cmd, "vim");
        assert_eq!(pane.pane_title, "~/code");
        assert_eq!(pane.width, 120);
        assert_eq!(pane.height, 40);
        assert!(pane.is_active);
    }

    #[test]
    fn parse_inactive_pane() {
        let line = "work\t@2\tshell\t%5\tzsh\t~\t80\t24\t0";
        let pane = parse_pane_line(line).unwrap();
        assert!(!pane.is_active);
        assert_eq!(pane.pane_id, "%5");
        assert_eq!(pane.width, 80);
        assert_eq!(pane.height, 24);
    }

    #[test]
    fn parse_multiple_lines() {
        let output = "\
main\t@0\teditor\t%1\tvim\t~/code\t120\t40\t1
main\t@0\teditor\t%2\tbash\t~/code\t120\t40\t0
work\t@1\tlogs\t%3\ttail\t/var/log\t200\t50\t0
";
        let panes = parse_list_panes_output(output).unwrap();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].pane_id, "%1");
        assert_eq!(panes[1].pane_id, "%2");
        assert_eq!(panes[2].pane_id, "%3");
        assert_eq!(panes[2].window_name, "logs");
        assert_eq!(panes[2].current_cmd, "tail");
    }

    #[test]
    fn parse_empty_output() {
        let panes = parse_list_panes_output("").unwrap();
        assert!(panes.is_empty());
    }

    #[test]
    fn parse_blank_lines_skipped() {
        let output = "\n  \nmain\t@0\ted\t%1\tzsh\t~\t80\t24\t1\n\n";
        let panes = parse_list_panes_output(output).unwrap();
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn malformed_line_skipped() {
        // Only 5 fields — should be silently skipped.
        let output = "bad\tline\tonly\tfive\tfields\nmain\t@0\ted\t%1\tzsh\t~\t80\t24\t1\n";
        let panes = parse_list_panes_output(output).unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].session_name, "main");
    }

    #[test]
    fn invalid_width_skipped() {
        let output = "s\t@0\tw\t%1\tcmd\ttitle\tWIDE\t24\t1\nmain\t@0\ted\t%2\tzsh\t~\t80\t24\t0\n";
        let panes = parse_list_panes_output(output).unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%2");
    }

    #[test]
    fn invalid_height_skipped() {
        let output = "s\t@0\tw\t%1\tcmd\ttitle\t80\tTALL\t1\nmain\t@0\ted\t%2\tzsh\t~\t80\t24\t0\n";
        let panes = parse_list_panes_output(output).unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%2");
    }

    #[test]
    fn pane_title_with_colon_works() {
        // With tab separators, colons in pane_title are no longer a problem.
        let line = "s\t@0\tw\t%1\tcmd\ttitle:with:colon\t80\t24\t1";
        let pane = parse_pane_line(line).unwrap();
        assert_eq!(pane.pane_title, "title:with:colon");
        assert_eq!(pane.width, 80);
        assert_eq!(pane.height, 24);
        assert!(pane.is_active);
    }

    #[test]
    fn parse_pane_line_error_on_too_few_fields() {
        let err = parse_pane_line("a\tb\tc").unwrap_err();
        match err {
            TmuxError::Parse(msg) => {
                assert!(msg.contains("expected 9 fields"), "msg was: {msg}");
            }
            other => panic!("expected Parse error, got: {other:?}"),
        }
    }
}
