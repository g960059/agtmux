//! Bridge TmuxPaneInfo + capture output + generation tracking into PaneSnapshot.

use agtmux_source_poller::source::PaneSnapshot;
use chrono::{DateTime, Utc};

use crate::capture::inspect_pane_processes;
use crate::generation::PaneGenerationTracker;
use crate::pane_info::TmuxPaneInfo;

/// Convert TmuxPaneInfo + captured lines into a PaneSnapshot for the poller.
pub fn to_pane_snapshot(
    pane: &TmuxPaneInfo,
    capture_lines: Vec<String>,
    _generation_tracker: &PaneGenerationTracker,
    now: DateTime<Utc>,
) -> PaneSnapshot {
    let process_hint = inspect_pane_processes(&pane.current_cmd);

    PaneSnapshot {
        pane_id: pane.pane_id.clone(),
        pane_title: pane.pane_title.clone(),
        current_cmd: pane.current_cmd.clone(),
        process_hint,
        capture_lines,
        captured_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid")
            .with_timezone(&Utc)
    }

    #[test]
    fn snapshot_from_claude_pane() {
        let pane = TmuxPaneInfo {
            pane_id: "%0".to_string(),
            pane_title: "claude code".to_string(),
            current_cmd: "claude".to_string(),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");

        let snapshot = to_pane_snapshot(&pane, vec!["hello".to_string()], &tracker, now);

        assert_eq!(snapshot.pane_id, "%0");
        assert_eq!(snapshot.pane_title, "claude code");
        assert_eq!(snapshot.current_cmd, "claude");
        assert_eq!(snapshot.process_hint, Some("claude".to_string()));
        assert_eq!(snapshot.capture_lines, vec!["hello"]);
        assert_eq!(snapshot.captured_at, now);
    }

    #[test]
    fn snapshot_no_agent_hint() {
        let pane = TmuxPaneInfo {
            pane_id: "%1".to_string(),
            current_cmd: "vim".to_string(),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now);

        assert_eq!(snapshot.process_hint, None);
    }

    #[test]
    fn snapshot_codex_hint() {
        let pane = TmuxPaneInfo {
            pane_id: "%2".to_string(),
            current_cmd: "codex --model o3".to_string(),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now);

        assert_eq!(snapshot.process_hint, Some("codex".to_string()));
    }
}
