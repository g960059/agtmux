use crate::display::state_indicator;
use crate::server::PaneInfo;

/// Format provider for display (dash for None).
fn format_provider(provider: &Option<String>) -> &str {
    match provider {
        Some(p) => p.as_str(),
        None => "—",
    }
}

/// Format a human-friendly activity state label.
fn format_state(activity_state: &str) -> &str {
    match activity_state {
        "running" => "Running",
        "waiting_approval" => "Approval",
        "waiting_input" => "Input",
        "idle" => "Idle",
        "error" => "Error",
        "unknown" => "Unknown",
        other => other,
    }
}

/// Build a summary line like "1 running, 1 approval, 1 idle".
fn format_summary(panes: &[PaneInfo]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let labels = [
        "error",
        "waiting_approval",
        "waiting_input",
        "running",
        "idle",
        "unknown",
    ];
    let display = ["error", "approval", "input", "running", "idle", "unknown"];

    for (label, display_name) in labels.iter().zip(display.iter()) {
        let count = panes
            .iter()
            .filter(|p| {
                if *label == "unknown" {
                    !labels[..5].contains(&p.activity_state.as_str())
                } else {
                    p.activity_state == *label
                }
            })
            .count();
        if count > 0 {
            counts.push((display_name, count));
        }
    }

    if counts.is_empty() {
        return "no panes".to_string();
    }

    counts
        .iter()
        .map(|(name, count)| format!("{} {}", count, name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format the full status output for `agtmux status`.
///
/// Example output:
/// ```text
/// AGTMUX Status
/// ─────────────────────────────────────────────────────────────
/// ● %1  claude   Running    (hook, 92%)   main:@1
/// ◉ %3  claude   Approval   (hook, 95%)   main:@2
/// ○ %5  codex    Idle       (poller, 80%) dev:@3
/// ◌ %7  —        Unknown    (poller, 50%) dev:@4
///
/// Summary: 1 running, 1 approval, 1 idle, 1 unknown
/// ```
pub fn format_status(panes: &[PaneInfo]) -> String {
    let mut out = String::new();

    out.push_str("AGTMUX Status\n");
    out.push_str("─────────────────────────────────────────────────────────────\n");

    if panes.is_empty() {
        out.push_str("  No panes detected.\n");
        return out;
    }

    for pane in panes {
        let indicator = state_indicator(&pane.activity_state);
        let provider = format_provider(&pane.provider);
        let state_label = format_state(&pane.activity_state);
        let confidence_pct = (pane.activity_confidence * 100.0) as u32;
        let session_info = if pane.session_name.is_empty() {
            String::new()
        } else {
            format!("{}:{}", pane.session_name, pane.window_id)
        };

        out.push_str(&format!(
            "{} {:<5} {:<8} {:<10} ({}, {}%) {}\n",
            indicator,
            pane.pane_id,
            provider,
            state_label,
            pane.activity_source,
            confidence_pct,
            session_info,
        ));
    }

    out.push('\n');
    out.push_str(&format!("Summary: {}\n", format_summary(panes)));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pane(
        pane_id: &str,
        provider: Option<&str>,
        activity_state: &str,
        source: &str,
        confidence: f64,
        session: &str,
        window: &str,
    ) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.into(),
            session_name: session.into(),
            window_id: window.into(),
            pane_title: String::new(),
            current_cmd: String::new(),
            provider: provider.map(String::from),
            provider_confidence: 0.0,
            activity_state: activity_state.into(),
            activity_confidence: confidence,
            activity_source: source.into(),
            attention_state: "none".into(),
            attention_reason: String::new(),
            attention_since: None,
            updated_at: String::new(),
        }
    }

    #[test]
    fn format_status_empty_panes() {
        let output = format_status(&[]);
        assert!(output.contains("AGTMUX Status"));
        assert!(output.contains("No panes detected"));
    }

    #[test]
    fn format_status_multiple_panes() {
        let panes = vec![
            make_pane("%1", Some("claude"), "running", "hook", 0.92, "main", "@1"),
            make_pane("%3", Some("claude"), "waiting_approval", "hook", 0.95, "main", "@2"),
            make_pane("%5", Some("codex"), "idle", "poller", 0.80, "dev", "@3"),
            make_pane("%7", None, "unknown", "poller", 0.50, "dev", "@4"),
        ];
        let output = format_status(&panes);
        assert!(output.contains("● %1"));
        assert!(output.contains("◉ %3"));
        assert!(output.contains("○ %5"));
        assert!(output.contains("◌ %7"));
        assert!(output.contains("Summary: 1 approval, 1 running, 1 idle, 1 unknown"));
    }

    #[test]
    fn format_summary_counts() {
        let panes = vec![
            make_pane("%1", None, "running", "hook", 0.9, "", ""),
            make_pane("%2", None, "running", "hook", 0.9, "", ""),
            make_pane("%3", None, "error", "hook", 0.9, "", ""),
        ];
        let summary = format_summary(&panes);
        assert_eq!(summary, "1 error, 2 running");
    }

    #[test]
    fn format_summary_empty() {
        let summary = format_summary(&[]);
        assert_eq!(summary, "no panes");
    }

    #[test]
    fn format_provider_display() {
        assert_eq!(format_provider(&Some("claude".into())), "claude");
        assert_eq!(format_provider(&None), "—");
    }

    #[test]
    fn format_status_no_session() {
        let panes = vec![make_pane("%1", Some("claude"), "running", "hook", 0.9, "", "")];
        let output = format_status(&panes);
        assert!(output.contains("● %1"));
        // No session:window trailing info
        assert!(!output.contains("main:"));
    }

    #[test]
    fn format_status_with_session() {
        let panes = vec![make_pane("%1", Some("claude"), "running", "hook", 0.9, "main", "@1")];
        let output = format_status(&panes);
        assert!(output.contains("main:@1"));
    }
}
