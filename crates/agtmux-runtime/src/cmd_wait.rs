//! `agtmux wait` — block until agent state condition is met.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::client::rpc_call;

/// Wait condition: what to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCondition {
    /// Wait until all managed panes are Idle/Error/Unknown
    Idle,
    /// Wait until no managed panes are in WaitingInput/WaitingApproval
    NoWaiting,
}

/// Check if the condition is met for a set of managed panes.
pub(crate) fn condition_met(panes: &[&serde_json::Value], condition: WaitCondition) -> bool {
    match condition {
        WaitCondition::Idle => {
            // All managed panes must be Idle, Error, Unknown, or have no activity_state
            panes.iter().all(|p| {
                matches!(
                    p["activity_state"].as_str(),
                    Some("Idle") | Some("Error") | Some("Unknown") | None
                )
            })
        }
        WaitCondition::NoWaiting => {
            // No managed panes in WaitingInput or WaitingApproval
            !panes.iter().any(|p| {
                matches!(
                    p["activity_state"].as_str(),
                    Some("WaitingInput") | Some("WaitingApproval")
                )
            })
        }
    }
}

/// Build a summary string of states for progress display.
fn state_summary(panes: &[&serde_json::Value]) -> String {
    let mut running = 0usize;
    let mut waiting = 0usize;
    let mut idle = 0usize;

    for pane in panes {
        match pane["activity_state"].as_str() {
            Some("Running") => running += 1,
            Some("WaitingInput") | Some("WaitingApproval") => waiting += 1,
            Some("Idle") => idle += 1,
            _ => {}
        }
    }

    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running} Running"));
    }
    if waiting > 0 {
        parts.push(format!("{waiting} Waiting"));
    }
    if idle > 0 {
        parts.push(format!("{idle} Idle"));
    }
    parts.join(", ")
}

/// Entry point for `agtmux wait`.
///
/// Returns an exit code:
/// - 0: condition met
/// - 1: timeout
/// - 2: daemon unreachable
/// - 3: interrupted (Ctrl-C)
pub async fn cmd_wait(
    socket_path: &str,
    condition: WaitCondition,
    session_filter: Option<&str>,
    timeout_secs: Option<u64>,
    quiet: bool,
) -> i32 {
    let is_tty = std::io::stderr().is_terminal();
    let start = Instant::now();

    loop {
        // Check for Ctrl-C before each poll
        let panes_result = tokio::select! {
            result = rpc_call(socket_path, "list_panes") => result,
            _ = tokio::signal::ctrl_c() => {
                if is_tty && !quiet {
                    eprintln!();
                }
                return 3;
            }
        };

        let panes = match panes_result {
            Ok(p) => p,
            Err(_) => {
                if !quiet {
                    eprintln!("Cannot connect to daemon");
                }
                return 2;
            }
        };

        let arr = panes.as_array().cloned().unwrap_or_default();

        // Filter to managed panes, optionally by session
        let managed: Vec<&serde_json::Value> = arr
            .iter()
            .filter(|p| p["presence"].as_str() == Some("managed"))
            .filter(|p| session_filter.is_none() || p["session_name"].as_str() == session_filter)
            .collect();

        if condition_met(&managed, condition) {
            let elapsed = start.elapsed().as_secs();
            let count = managed.len();
            if is_tty && !quiet {
                let msg = match condition {
                    WaitCondition::Idle => {
                        format!("All {count} agents idle. ({elapsed}s)")
                    }
                    WaitCondition::NoWaiting => {
                        format!("No agents waiting. ({elapsed}s)")
                    }
                };
                eprintln!("\r{msg}");
            }
            return 0;
        }

        // Progress display
        if is_tty && !quiet {
            let elapsed = start.elapsed().as_secs();
            let summary = state_summary(&managed);
            let elapsed_display = if elapsed >= 60 {
                format!("{}m{:02}s", elapsed / 60, elapsed % 60)
            } else {
                format!("{elapsed}s")
            };
            eprint!("\rWaiting... {summary} ({elapsed_display})");
            let _ = std::io::stderr().flush();
        }

        // Timeout check
        if let Some(timeout) = timeout_secs {
            if start.elapsed().as_secs() >= timeout {
                if is_tty && !quiet {
                    eprintln!("\rTimeout after {timeout}s");
                }
                return 1;
            }
        }

        // Sleep or interrupt
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = tokio::signal::ctrl_c() => {
                if is_tty && !quiet {
                    eprintln!();
                }
                return 3;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_managed_pane(activity_state: &str) -> serde_json::Value {
        serde_json::json!({
            "pane_id": "%0",
            "session_name": "work",
            "session_id": "$0",
            "window_id": "@0",
            "window_name": "dev",
            "presence": "managed",
            "evidence_mode": "deterministic",
            "activity_state": activity_state,
            "current_cmd": "claude",
            "current_path": "/repo",
        })
    }

    #[test]
    fn wait_condition_idle_all_idle() {
        let p1 = make_managed_pane("Idle");
        let p2 = make_managed_pane("Idle");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(condition_met(&panes, WaitCondition::Idle));
    }

    #[test]
    fn wait_condition_idle_with_error() {
        let p1 = make_managed_pane("Idle");
        let p2 = make_managed_pane("Error");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(condition_met(&panes, WaitCondition::Idle));
    }

    #[test]
    fn wait_condition_idle_one_running() {
        let p1 = make_managed_pane("Idle");
        let p2 = make_managed_pane("Running");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(!condition_met(&panes, WaitCondition::Idle));
    }

    #[test]
    fn wait_condition_idle_empty_is_met() {
        let panes: Vec<&serde_json::Value> = vec![];
        assert!(condition_met(&panes, WaitCondition::Idle));
    }

    #[test]
    fn wait_condition_no_waiting_none() {
        let p1 = make_managed_pane("Running");
        let p2 = make_managed_pane("Idle");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(condition_met(&panes, WaitCondition::NoWaiting));
    }

    #[test]
    fn wait_condition_no_waiting_has_waiting() {
        let p1 = make_managed_pane("Running");
        let p2 = make_managed_pane("WaitingApproval");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(!condition_met(&panes, WaitCondition::NoWaiting));
    }

    #[test]
    fn wait_condition_no_waiting_has_waiting_input() {
        let p1 = make_managed_pane("Idle");
        let p2 = make_managed_pane("WaitingInput");
        let panes: Vec<&serde_json::Value> = vec![&p1, &p2];
        assert!(!condition_met(&panes, WaitCondition::NoWaiting));
    }

    #[test]
    fn wait_condition_no_waiting_empty() {
        let panes: Vec<&serde_json::Value> = vec![];
        assert!(condition_met(&panes, WaitCondition::NoWaiting));
    }
}
