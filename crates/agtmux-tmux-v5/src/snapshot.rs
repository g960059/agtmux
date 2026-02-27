//! Bridge TmuxPaneInfo + capture output + generation tracking into PaneSnapshot.

use agtmux_source_poller::source::PaneSnapshot;
use chrono::{DateTime, Utc};

use crate::capture::{ProcessMap, inspect_pane_processes, inspect_pane_processes_deep};
use crate::generation::PaneGenerationTracker;
use crate::pane_info::TmuxPaneInfo;

/// Convert TmuxPaneInfo + captured lines into a PaneSnapshot for the poller.
///
/// When `process_map` is provided and `pane.pane_pid` is known, performs deep
/// process-tree inspection (`inspect_pane_processes_deep`) to distinguish agents
/// that share the same parent runtime (e.g. Codex vs Claude Code under `node`).
/// Falls back to shallow `inspect_pane_processes` otherwise.
pub fn to_pane_snapshot(
    pane: &TmuxPaneInfo,
    capture_lines: Vec<String>,
    _generation_tracker: &PaneGenerationTracker,
    now: DateTime<Utc>,
    process_map: Option<&ProcessMap>,
) -> PaneSnapshot {
    let process_hint = match (pane.pane_pid, process_map) {
        (Some(pid), Some(pm)) => inspect_pane_processes_deep(&pane.current_cmd, pid, pm),
        _ => inspect_pane_processes(&pane.current_cmd),
    };

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

        let snapshot = to_pane_snapshot(&pane, vec!["hello".to_string()], &tracker, now, None);

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
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, None);

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
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, None);

        assert_eq!(snapshot.process_hint, Some("codex".to_string()));
    }

    #[test]
    fn snapshot_deep_inspection_claude_child() {
        // pane_pid=10 (node), child pid=11 has claude in argv → Some("claude")
        use crate::capture::{ProcessInfo, ProcessMap};
        let mut pm: ProcessMap = ProcessMap::new();
        pm.insert(
            10,
            ProcessInfo {
                pid: 10,
                ppid: 1,
                args: "node".to_string(),
            },
        );
        pm.insert(
            11,
            ProcessInfo {
                pid: 11,
                ppid: 10,
                args: "/usr/local/bin/node /Users/user/.config/claude/cli.js".to_string(),
            },
        );
        let pane = TmuxPaneInfo {
            pane_id: "%5".to_string(),
            current_cmd: "node".to_string(),
            pane_pid: Some(10),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, Some(&pm));
        assert_eq!(snapshot.process_hint, Some("claude".to_string()));
    }

    #[test]
    fn snapshot_deep_inspection_codex_child() {
        use crate::capture::{ProcessInfo, ProcessMap};
        let mut pm: ProcessMap = ProcessMap::new();
        pm.insert(
            20,
            ProcessInfo {
                pid: 20,
                ppid: 1,
                args: "node".to_string(),
            },
        );
        pm.insert(
            21,
            ProcessInfo {
                pid: 21,
                ppid: 20,
                args: "node /usr/local/lib/node_modules/@openai/codex/dist/cli.mjs exec"
                    .to_string(),
            },
        );
        let pane = TmuxPaneInfo {
            pane_id: "%6".to_string(),
            current_cmd: "node".to_string(),
            pane_pid: Some(20),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, Some(&pm));
        assert_eq!(snapshot.process_hint, Some("codex".to_string()));
    }

    #[test]
    fn snapshot_deep_inspection_runtime_unknown() {
        // Child exists but argv contains neither "claude" nor "codex" → runtime_unknown
        use crate::capture::{ProcessInfo, ProcessMap};
        let mut pm: ProcessMap = ProcessMap::new();
        pm.insert(
            30,
            ProcessInfo {
                pid: 30,
                ppid: 1,
                args: "node".to_string(),
            },
        );
        pm.insert(
            31,
            ProcessInfo {
                pid: 31,
                ppid: 30,
                args: "node /path/to/some-other-tool/index.js".to_string(),
            },
        );
        let pane = TmuxPaneInfo {
            pane_id: "%7".to_string(),
            current_cmd: "node".to_string(),
            pane_pid: Some(30),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, Some(&pm));
        assert_eq!(snapshot.process_hint, Some("runtime_unknown".to_string()));
    }

    #[test]
    fn snapshot_deep_inspection_no_children_falls_back() {
        // pane_pid present but no children in process_map → shallow fallback (None for node)
        use crate::capture::{ProcessInfo, ProcessMap};
        let mut pm: ProcessMap = ProcessMap::new();
        pm.insert(
            40,
            ProcessInfo {
                pid: 40,
                ppid: 1,
                args: "node".to_string(),
            },
        );
        // no children of pid 40
        let pane = TmuxPaneInfo {
            pane_id: "%8".to_string(),
            current_cmd: "node".to_string(),
            pane_pid: Some(40),
            ..Default::default()
        };
        let tracker = PaneGenerationTracker::new();
        let now = ts("2026-02-25T12:00:00Z");
        let snapshot = to_pane_snapshot(&pane, vec![], &tracker, now, Some(&pm));
        assert_eq!(snapshot.process_hint, None);
    }
}
