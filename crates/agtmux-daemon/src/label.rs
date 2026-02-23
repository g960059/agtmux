use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A recorded event from the JSONL file (minimal structure).
///
/// Works with any event type. Only `StateChanged` events are presented for
/// labeling; other event types are preserved as-is in the output.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordedEvent {
    /// Timestamp string (ISO 8601).
    pub ts: String,
    /// Event type, e.g. "StateChanged", "PaneAdded", "PaneRemoved".
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event payload (opaque JSON value).
    pub data: serde_json::Value,
    /// Optional human label assigned during labeling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Information extracted from a StateChanged event for display.
struct DisplayInfo {
    pane_id: String,
    predicted_state: String,
    confidence: String,
    reason_code: String,
}

/// Extract display-relevant fields from a RecordedEvent's data.
fn extract_display_info(event: &RecordedEvent) -> DisplayInfo {
    let pane_id = event
        .data
        .get("pane_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    let state_obj = event.data.get("state");

    let predicted_state = state_obj
        .and_then(|s| s.get("activity"))
        .and_then(|a| a.get("state"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    let confidence = state_obj
        .and_then(|s| s.get("activity"))
        .and_then(|a| a.get("confidence"))
        .and_then(|v| v.as_f64())
        .map(|c| format!("{:.0}%", c * 100.0))
        .unwrap_or_else(|| "?".to_string());

    let reason_code = state_obj
        .and_then(|s| s.get("activity"))
        .and_then(|a| a.get("reason_code"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    DisplayInfo {
        pane_id,
        predicted_state,
        confidence,
        reason_code,
    }
}

/// Map a key character to a label string.
fn key_to_label(c: char) -> Option<&'static str> {
    match c {
        'r' => Some("running"),
        'a' => Some("waiting_approval"),
        'i' => Some("waiting_input"),
        'd' => Some("idle"),
        'e' => Some("error"),
        'u' => Some("unknown"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Terminal cleanup guard (same pattern as tui.rs)
// ---------------------------------------------------------------------------

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the labeling TUI on a recorded JSONL file.
///
/// Reads all events, presents each `StateChanged` event one at a time, and
/// collects a human label via key press. Writes the labeled output to
/// `<input>.labeled.jsonl`.
pub fn run_label(input_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Read all events from JSONL.
    let file = std::fs::File::open(input_path)?;
    let reader = BufReader::new(file);
    let mut events: Vec<RecordedEvent> = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }
        let event: RecordedEvent = serde_json::from_str(&line).map_err(|e| {
            format!("line {}: failed to parse JSON: {}", line_num + 1, e)
        })?;
        events.push(event);
    }

    // 2. Collect indices of StateChanged events to label.
    let labelable_indices: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.event_type == "StateChanged")
        .map(|(i, _)| i)
        .collect();

    if labelable_indices.is_empty() {
        eprintln!("No StateChanged events found in {:?}", input_path);
        return Ok(());
    }

    let total = labelable_indices.len();
    let mut current = 0; // index into labelable_indices
    let mut labeled_count = 0usize;
    let mut skipped_count = 0usize;

    // 3. Setup terminal.
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 4. Main loop: present each event.
    while current < total {
        let event_idx = labelable_indices[current];
        let event = &events[event_idx];
        let info = extract_display_info(event);

        // Render
        terminal.draw(|frame| {
            render_label_screen(frame, current, total, event, &info);
        })?;

        // Wait for key press.
        loop {
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            // Quit early -- still write what we have.
                            terminal.show_cursor()?;
                            drop(_guard);
                            // Fall through to write output.
                            return write_output(input_path, &events, labeled_count, skipped_count);
                        }
                        (KeyCode::Char('s'), _) => {
                            // Skip
                            skipped_count += 1;
                            current += 1;
                            break;
                        }
                        (KeyCode::Char(c), _) => {
                            if let Some(label) = key_to_label(c) {
                                events[event_idx].label = Some(label.to_string());
                                labeled_count += 1;
                                current += 1;
                                break;
                            }
                            // Ignore unrecognized keys.
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // 5. Cleanup terminal.
    terminal.show_cursor()?;
    // _guard will restore raw mode and alternate screen on drop.

    // 6. Write output.
    // We need to drop the guard before printing to stdout.
    // The guard drops at the end of scope, but we want to print the summary
    // after cleanup. So we do it in the helper.
    drop(_guard);
    write_output(input_path, &events, labeled_count, skipped_count)
}

/// Write the labeled events to the output file and print a summary.
fn write_output(
    input_path: &Path,
    events: &[RecordedEvent],
    labeled_count: usize,
    skipped_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Restore terminal state before printing.
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);

    let output_path = {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("recording");
        let parent = input_path.parent().unwrap_or_else(|| Path::new("."));
        parent.join(format!("{}.labeled.jsonl", stem))
    };

    let mut out_file = std::fs::File::create(&output_path)?;
    for event in events {
        let line = serde_json::to_string(event)?;
        writeln!(out_file, "{}", line)?;
    }

    println!("Output: {:?}", output_path);
    println!("Labeled: {}, Skipped: {}", labeled_count, skipped_count);

    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_label_screen(
    frame: &mut Frame,
    current: usize,
    total: usize,
    event: &RecordedEvent,
    info: &DisplayInfo,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),  // Event details
            Constraint::Length(5), // Key help
        ])
        .split(frame.area());

    // Title bar with progress.
    let title = Paragraph::new(format!(
        " AGTMUX Label  [{}/{}]",
        current + 1,
        total
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(title, chunks[0]);

    // Event details.
    let details = vec![
        Line::from(vec![
            Span::styled("Timestamp:  ", Style::default().bold()),
            Span::raw(&event.ts),
        ]),
        Line::from(vec![
            Span::styled("Pane ID:    ", Style::default().bold()),
            Span::raw(&info.pane_id),
        ]),
        Line::from(vec![
            Span::styled("Predicted:  ", Style::default().bold()),
            Span::styled(
                &info.predicted_state,
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("Confidence: ", Style::default().bold()),
            Span::raw(&info.confidence),
        ]),
        Line::from(vec![
            Span::styled("Reason:     ", Style::default().bold()),
            Span::raw(&info.reason_code),
        ]),
    ];
    let details_widget = Paragraph::new(details).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Event Details "),
    );
    frame.render_widget(details_widget, chunks[1]);

    // Key help.
    let help_text = vec![
        Line::from(" r=Running  a=WaitingApproval  i=WaitingInput"),
        Line::from(" d=Idle     e=Error            u=Unknown      s=Skip"),
        Line::from(" q=Quit"),
    ];
    let help = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Assign Label "),
    );
    frame.render_widget(help, chunks[2]);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_event_roundtrip_without_label() {
        let event = RecordedEvent {
            ts: "2026-01-01T00:00:01Z".to_string(),
            event_type: "StateChanged".to_string(),
            data: serde_json::json!({"pane_id": "%1", "state": {}}),
            label: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: RecordedEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.ts, "2026-01-01T00:00:01Z");
        assert_eq!(parsed.event_type, "StateChanged");
        assert!(parsed.label.is_none());

        // The "label" field should not appear in the JSON when None.
        assert!(!json.contains("\"label\""));
    }

    #[test]
    fn recorded_event_roundtrip_with_label() {
        let event = RecordedEvent {
            ts: "2026-01-01T00:00:01Z".to_string(),
            event_type: "StateChanged".to_string(),
            data: serde_json::json!({"pane_id": "%1", "state": {}}),
            label: Some("running".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: RecordedEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.label, Some("running".to_string()));

        // The "label" field should appear in the JSON.
        assert!(json.contains("\"label\":\"running\""));
    }

    #[test]
    fn recorded_event_deserialize_without_label_field() {
        // JSONL from daemon recording won't have a label field at all.
        let json = r#"{"ts":"2026-01-01T00:00:01Z","type":"StateChanged","data":{"pane_id":"%1"}}"#;
        let event: RecordedEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.ts, "2026-01-01T00:00:01Z");
        assert_eq!(event.event_type, "StateChanged");
        assert!(event.label.is_none());
    }

    #[test]
    fn recorded_event_deserialize_with_label_field() {
        let json = r#"{"ts":"2026-01-01T00:00:01Z","type":"StateChanged","data":{},"label":"idle"}"#;
        let event: RecordedEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.label, Some("idle".to_string()));
    }

    #[test]
    fn recorded_event_preserves_extra_data_fields() {
        let json = r#"{"ts":"2026-01-01T00:00:01Z","type":"StateChanged","data":{"pane_id":"%1","state":{"activity":{"state":"running","confidence":0.95,"source":"hook","reason_code":"tool_running"}}}}"#;
        let event: RecordedEvent = serde_json::from_str(json).unwrap();
        let info = extract_display_info(&event);
        assert_eq!(info.pane_id, "%1");
        assert_eq!(info.predicted_state, "running");
        assert_eq!(info.confidence, "95%");
        assert_eq!(info.reason_code, "tool_running");
    }

    #[test]
    fn extract_display_info_missing_fields() {
        let event = RecordedEvent {
            ts: "2026-01-01T00:00:01Z".to_string(),
            event_type: "StateChanged".to_string(),
            data: serde_json::json!({}),
            label: None,
        };
        let info = extract_display_info(&event);
        assert_eq!(info.pane_id, "?");
        assert_eq!(info.predicted_state, "?");
        assert_eq!(info.confidence, "?");
        assert_eq!(info.reason_code, "");
    }

    #[test]
    fn key_to_label_mapping() {
        assert_eq!(key_to_label('r'), Some("running"));
        assert_eq!(key_to_label('a'), Some("waiting_approval"));
        assert_eq!(key_to_label('i'), Some("waiting_input"));
        assert_eq!(key_to_label('d'), Some("idle"));
        assert_eq!(key_to_label('e'), Some("error"));
        assert_eq!(key_to_label('u'), Some("unknown"));
        assert_eq!(key_to_label('x'), None);
        assert_eq!(key_to_label('s'), None); // 's' is skip, not a label
    }

    #[test]
    fn labeled_event_serialization_includes_label() {
        let event = RecordedEvent {
            ts: "2026-01-01T00:00:05Z".to_string(),
            event_type: "StateChanged".to_string(),
            data: serde_json::json!({
                "pane_id": "%2",
                "state": {
                    "activity": {
                        "state": "waiting_input",
                        "confidence": 0.88
                    }
                }
            }),
            label: Some("waiting_input".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"label\":\"waiting_input\""));
        assert!(json.contains("\"type\":\"StateChanged\""));

        // Round-trip
        let parsed: RecordedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.label, Some("waiting_input".to_string()));
        assert_eq!(parsed.event_type, "StateChanged");
    }
}
