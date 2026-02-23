use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::display::state_indicator;
use crate::server::PaneInfo;

// ---------------------------------------------------------------------------
// Terminal cleanup guard
// ---------------------------------------------------------------------------

/// RAII guard that restores the terminal to its normal state when dropped.
/// This ensures cleanup happens even on panic or early `?` returns.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// App state for the TUI.
struct App {
    panes: Vec<PaneInfo>,
    selected: usize,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            panes: Vec::new(),
            selected: 0,
            should_quit: false,
        }
    }

    /// Move selection down (j / Down).
    fn next(&mut self) {
        if !self.panes.is_empty() {
            self.selected = (self.selected + 1).min(self.panes.len() - 1);
        }
    }

    /// Move selection up (k / Up).
    fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Replace the pane list and clamp the selection index.
    fn update_panes(&mut self, panes: Vec<PaneInfo>) {
        self.panes = panes;
        if self.panes.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.panes.len() {
            self.selected = self.panes.len() - 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon communication helpers
// ---------------------------------------------------------------------------

/// Send a newline-delimited JSON-RPC request.
async fn send_request(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> io::Result<()> {
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut buf = serde_json::to_vec(&req)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    buf.push(b'\n');
    writer.write_all(&buf).await?;
    writer.flush().await
}

/// Parse a `list_panes` response into a `Vec<PaneInfo>`.
fn parse_list_panes_response(line: &str) -> Option<Vec<PaneInfo>> {
    let val: serde_json::Value = serde_json::from_str(line).ok()?;
    // Accept both response (has "result") and notification formats.
    let panes_val = val
        .get("result")
        .and_then(|r| r.get("panes"))
        .or_else(|| val.get("params").and_then(|p| p.get("panes")))?;
    serde_json::from_value(panes_val.clone()).ok()
}

/// Check whether a JSON line is a push notification that should trigger a
/// refresh (state_changed, pane_added, pane_removed).
fn is_refresh_notification(line: &str) -> bool {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
            return matches!(method, "state_changed" | "pane_added" | "pane_removed");
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the TUI application.
///
/// Connects to the daemon at `socket_path`, subscribes to live updates, and
/// renders an interactive pane list until the user presses `q` or Ctrl+C.
pub async fn run_tui(socket_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Connect to daemon
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // 2. Send initial requests: list_panes + subscribe
    send_request(&mut writer, 1, "list_panes", serde_json::json!({})).await?;
    send_request(
        &mut writer,
        2,
        "subscribe",
        serde_json::json!({"events": ["state", "topology"]}),
    )
    .await?;

    // 3. Read the list_panes response to get initial state
    let mut app = App::new();
    // We may receive the subscribe response first, so read lines until we
    // get the list_panes result.
    let mut initial_loaded = false;
    while !initial_loaded {
        if let Some(line) = lines.next_line().await? {
            if let Some(panes) = parse_list_panes_response(&line) {
                app.update_panes(panes);
                initial_loaded = true;
            }
        } else {
            return Err("daemon closed connection before sending initial state".into());
        }
    }

    // 4. Setup terminal — the TerminalGuard ensures cleanup on panic or early return.
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Track the next JSON-RPC request ID.
    let mut next_id: u64 = 3;

    // 5. Main loop
    loop {
        // Render
        terminal.draw(|frame| render(frame, &app))?;

        if app.should_quit {
            break;
        }

        // Interleave keyboard events and daemon messages
        tokio::select! {
            // Short timeout for crossterm event polling so we remain responsive.
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // Check for keyboard events (non-blocking).
                while event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        match (key.code, key.modifiers) {
                            (KeyCode::Char('q'), _) => {
                                app.should_quit = true;
                            }
                            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                app.should_quit = true;
                            }
                            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                app.next();
                            }
                            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                app.previous();
                            }
                            (KeyCode::Char('r'), _) => {
                                // Manual refresh
                                send_request(&mut writer, next_id, "list_panes", serde_json::json!({})).await?;
                                next_id += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Read daemon push notifications
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        // If it is a list_panes response, update directly.
                        if let Some(panes) = parse_list_panes_response(&line) {
                            app.update_panes(panes);
                        } else if is_refresh_notification(&line) {
                            // A state or topology change happened — re-query.
                            send_request(&mut writer, next_id, "list_panes", serde_json::json!({})).await?;
                            next_id += 1;
                        }
                    }
                    Ok(None) => {
                        // Daemon disconnected
                        app.should_quit = true;
                    }
                    Err(_) => {
                        app.should_quit = true;
                    }
                }
            }
        }
    }

    // 6. Cleanup terminal — TerminalGuard handles raw mode and alternate screen.
    // We explicitly show the cursor as well.
    terminal.show_cursor()?;
    // _guard will be dropped here, restoring the terminal.

    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the full TUI frame.
fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title bar
            Constraint::Min(5),   // Pane list
            Constraint::Length(3), // Help bar
        ])
        .split(frame.area());

    // Title
    let title = Block::default()
        .title(" AGTMUX ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(title, chunks[0]);

    // Pane list as a Table
    let header = Row::new(vec![
        "", "Pane", "Provider", "State", "Source", "Conf", "Attention", "Session",
    ])
    .style(Style::default().bold());

    let rows: Vec<Row> = app
        .panes
        .iter()
        .enumerate()
        .map(|(i, pane)| {
            let indicator = state_indicator(&pane.activity_state);
            let style = if i == app.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            Row::new(vec![
                indicator.to_string(),
                pane.pane_id.clone(),
                pane.provider
                    .clone()
                    .unwrap_or_else(|| "\u{2014}".into()), // em-dash
                pane.activity_state.clone(),
                pane.activity_source.clone(),
                format!("{:.0}%", pane.activity_confidence * 100.0),
                pane.attention_state.clone(),
                pane.session_name.clone(),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),  // indicator
            Constraint::Length(6),  // pane_id
            Constraint::Length(10), // provider
            Constraint::Length(18), // state
            Constraint::Length(8),  // source
            Constraint::Length(6),  // confidence
            Constraint::Length(24), // attention
            Constraint::Length(12), // session
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Panes "));
    frame.render_widget(table, chunks[1]);

    // Help bar
    let help = Paragraph::new(" j/k: navigate | q: quit | r: refresh")
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(help, chunks[2]);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a minimal PaneInfo for testing.
    fn make_pane(id: &str, state: &str) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            session_name: "main".into(),
            window_id: "@1".into(),
            pane_title: "".into(),
            current_cmd: "claude".into(),
            provider: Some("claude".into()),
            provider_confidence: 0.95,
            activity_state: state.into(),
            activity_confidence: 0.9,
            activity_source: "hook".into(),
            attention_state: "none".into(),
            attention_reason: "".into(),
            attention_since: None,
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    // -----------------------------------------------------------------------
    // App::next / App::previous navigation tests
    // -----------------------------------------------------------------------

    #[test]
    fn next_increments_selection() {
        let mut app = App::new();
        app.update_panes(vec![
            make_pane("%1", "running"),
            make_pane("%2", "idle"),
            make_pane("%3", "error"),
        ]);
        assert_eq!(app.selected, 0);

        app.next();
        assert_eq!(app.selected, 1);

        app.next();
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn next_clamps_at_last_element() {
        let mut app = App::new();
        app.update_panes(vec![make_pane("%1", "running"), make_pane("%2", "idle")]);

        app.next();
        app.next();
        app.next(); // should not go beyond 1
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn next_noop_on_empty() {
        let mut app = App::new();
        app.next();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn previous_decrements_selection() {
        let mut app = App::new();
        app.update_panes(vec![
            make_pane("%1", "running"),
            make_pane("%2", "idle"),
            make_pane("%3", "error"),
        ]);
        app.selected = 2;

        app.previous();
        assert_eq!(app.selected, 1);

        app.previous();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn previous_clamps_at_zero() {
        let mut app = App::new();
        app.update_panes(vec![make_pane("%1", "running")]);

        app.previous();
        assert_eq!(app.selected, 0);

        app.previous(); // already at 0
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn previous_noop_on_empty() {
        let mut app = App::new();
        app.previous();
        assert_eq!(app.selected, 0);
    }

    // -----------------------------------------------------------------------
    // App::update_panes tests
    // -----------------------------------------------------------------------

    #[test]
    fn update_panes_sets_panes() {
        let mut app = App::new();
        let panes = vec![make_pane("%1", "running"), make_pane("%2", "idle")];
        app.update_panes(panes);
        assert_eq!(app.panes.len(), 2);
    }

    #[test]
    fn update_panes_clamps_selection_when_list_shrinks() {
        let mut app = App::new();
        app.update_panes(vec![
            make_pane("%1", "running"),
            make_pane("%2", "idle"),
            make_pane("%3", "error"),
        ]);
        app.selected = 2; // pointing at %3

        // Shrink to 2 panes: selection should clamp to 1
        app.update_panes(vec![make_pane("%1", "running"), make_pane("%2", "idle")]);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn update_panes_clamps_selection_to_zero_for_single_pane() {
        let mut app = App::new();
        app.selected = 5; // artificially large
        app.update_panes(vec![make_pane("%1", "running")]);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn update_panes_empty_list_resets_selection_to_zero() {
        let mut app = App::new();
        app.selected = 3;
        app.update_panes(Vec::new());
        assert_eq!(app.panes.len(), 0);
        assert_eq!(app.selected, 0);
    }

    // -----------------------------------------------------------------------
    // parse_list_panes_response tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_valid_list_panes_response() {
        let json = r#"{"id":1,"result":{"panes":[{"pane_id":"%1","session_name":"s","window_id":"@1","pane_title":"","current_cmd":"claude","provider":"claude","provider_confidence":0.95,"activity_state":"running","activity_confidence":0.9,"activity_source":"hook","attention_state":"none","attention_reason":"","attention_since":null,"updated_at":"2026-01-01T00:00:00Z"}]}}"#;
        let panes = parse_list_panes_response(json);
        assert!(panes.is_some());
        let panes = panes.unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%1");
    }

    #[test]
    fn parse_non_list_panes_response_returns_none() {
        let json = r#"{"id":2,"result":{"subscribed":true}}"#;
        assert!(parse_list_panes_response(json).is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_list_panes_response("not json").is_none());
    }

    // -----------------------------------------------------------------------
    // is_refresh_notification tests
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_notification_state_changed() {
        let json = r#"{"method":"state_changed","params":{"pane_id":"%1"}}"#;
        assert!(is_refresh_notification(json));
    }

    #[test]
    fn refresh_notification_pane_added() {
        let json = r#"{"method":"pane_added","params":{"pane_id":"%1"}}"#;
        assert!(is_refresh_notification(json));
    }

    #[test]
    fn refresh_notification_pane_removed() {
        let json = r#"{"method":"pane_removed","params":{"pane_id":"%2"}}"#;
        assert!(is_refresh_notification(json));
    }

    #[test]
    fn non_refresh_notification_returns_false() {
        let json = r#"{"method":"summary","params":{"counts":{}}}"#;
        assert!(!is_refresh_notification(json));
    }

    #[test]
    fn refresh_notification_invalid_json_returns_false() {
        assert!(!is_refresh_notification("not json"));
    }
}
