use crate::server::PaneInfo;

/// Format pane state for tmux status line.
///
/// Output format: compact, single-line, designed for `#(agtmux tmux-status)`.
///
/// Examples:
///   "●2 ◉1"           -- 2 running, 1 approval
///   "●1 ◉1 ✖1"       -- 1 running, 1 approval, 1 error
///   "○3"               -- 3 idle
///   ""                 -- no panes / daemon not running
///
/// Indicators:
///   ✖ Error
///   ◉ WaitingApproval (highest priority -- attention needed)
///   ◈ WaitingInput
///   ● Running
///   ○ Idle
///   ◌ Unknown
pub fn format_tmux_status(panes: &[PaneInfo]) -> String {
    if panes.is_empty() {
        return String::new();
    }

    let mut error = 0usize;
    let mut approval = 0usize;
    let mut input = 0usize;
    let mut running = 0usize;
    let mut idle = 0usize;
    let mut unknown = 0usize;

    for pane in panes {
        match pane.activity_state.as_str() {
            "error" => error += 1,
            "waiting_approval" => approval += 1,
            "waiting_input" => input += 1,
            "running" => running += 1,
            "idle" => idle += 1,
            _ => unknown += 1,
        }
    }

    // Ordered by urgency: error > approval > input > running > idle > unknown
    let parts: Vec<String> = [
        ("✖", error),
        ("◉", approval),
        ("◈", input),
        ("●", running),
        ("○", idle),
        ("◌", unknown),
    ]
    .iter()
    .filter(|(_, count)| *count > 0)
    .map(|(indicator, count)| format!("{}{}", indicator, count))
    .collect();

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a PaneInfo with only the activity_state field set
    /// (the rest are irrelevant for tmux status formatting).
    fn pane(activity_state: &str) -> PaneInfo {
        PaneInfo {
            pane_id: "%0".into(),
            session_name: String::new(),
            window_id: String::new(),
            pane_title: String::new(),
            current_cmd: String::new(),
            provider: None,
            provider_confidence: 0.0,
            activity_state: activity_state.into(),
            activity_confidence: 0.0,
            activity_source: String::new(),
            attention_state: "none".into(),
            attention_reason: String::new(),
            attention_since: None,
            updated_at: String::new(),
        }
    }

    #[test]
    fn empty_panes_returns_empty_string() {
        assert_eq!(format_tmux_status(&[]), "");
    }

    #[test]
    fn single_running_pane() {
        let panes = vec![pane("running")];
        assert_eq!(format_tmux_status(&panes), "●1");
    }

    #[test]
    fn mixed_states_ordered_by_urgency() {
        let panes = vec![
            pane("running"),
            pane("waiting_approval"),
            pane("error"),
            pane("running"),
        ];
        // error first, then approval, then running
        assert_eq!(format_tmux_status(&panes), "✖1 ◉1 ●2");
    }

    #[test]
    fn only_idle_panes() {
        let panes = vec![pane("idle"), pane("idle"), pane("idle")];
        assert_eq!(format_tmux_status(&panes), "○3");
    }

    #[test]
    fn all_states_present() {
        let panes = vec![
            pane("error"),
            pane("waiting_approval"),
            pane("waiting_input"),
            pane("running"),
            pane("idle"),
            pane("unknown"),
        ];
        assert_eq!(format_tmux_status(&panes), "✖1 ◉1 ◈1 ●1 ○1 ◌1");
    }

    #[test]
    fn unknown_and_idle_not_shown_when_others_present() {
        // When there are active states, idle/unknown still appear
        // (this test verifies ordering; all non-zero counts are shown)
        let panes = vec![
            pane("running"),
            pane("running"),
            pane("idle"),
            pane("unknown"),
        ];
        assert_eq!(format_tmux_status(&panes), "●2 ○1 ◌1");
    }

    #[test]
    fn unrecognized_state_counts_as_unknown() {
        let panes = vec![pane("some_future_state")];
        assert_eq!(format_tmux_status(&panes), "◌1");
    }

    #[test]
    fn multiple_errors() {
        let panes = vec![pane("error"), pane("error"), pane("waiting_approval")];
        assert_eq!(format_tmux_status(&panes), "✖2 ◉1");
    }
}
