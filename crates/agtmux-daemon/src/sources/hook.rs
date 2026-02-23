use std::path::PathBuf;
use std::time::Duration;

use agtmux_core::source::SourceEvent;
use agtmux_core::types::{
    ActivityState, Evidence, EvidenceKind, Provider, SourceType,
};
use chrono::Utc;
use serde::Deserialize;
use tokio::io::AsyncBufReadExt;
use tokio::net::UnixListener;
use tokio::sync::mpsc;

/// JSON payload sent by Claude Code / agent hooks.
///
/// Example:
/// ```json
/// {"event": "PreToolUse", "pane_id": "%1", "payload": {"tool": "Bash"}}
/// ```
#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub event: String,
    pub pane_id: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Receives JSON hook events on a Unix stream socket and converts them
/// to `SourceEvent`s for the orchestrator pipeline.
pub struct HookSource {
    tx: mpsc::Sender<SourceEvent>,
    socket_path: PathBuf,
}

impl HookSource {
    pub fn new(tx: mpsc::Sender<SourceEvent>, socket_path: PathBuf) -> Self {
        Self { tx, socket_path }
    }

    /// Listen for hook events on a Unix stream socket.
    /// Each connection sends newline-delimited JSON. Blocks until cancelled.
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Remove stale socket file if it exists.
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path.display(), "hook source listening");

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let tx = self.tx.clone();

                    tokio::spawn(async move {
                        let reader = tokio::io::BufReader::new(stream);
                        let mut lines = reader.lines();

                        while let Ok(Some(line)) = lines.next_line().await {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            match serde_json::from_str::<HookEvent>(&line) {
                                Ok(hook) => {
                                    let event = hook_to_source_event(&hook);
                                    if let Err(e) = tx.send(event).await {
                                        tracing::warn!(
                                            pane_id = %hook.pane_id,
                                            "failed to send hook event: {e}"
                                        );
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("failed to parse hook JSON: {e}, line: {line}");
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("hook accept error: {e}");
                    continue;
                }
            }
        }
    }
}

/// Map a hook event to an appropriate `SourceEvent`.
///
/// Hook events from Claude Code carry rich semantic information (e.g.
/// PreToolUse, PostToolUse, Stop) that we translate into `Evidence`
/// with provider-appropriate signal, weight, and confidence values.
fn hook_to_source_event(hook: &HookEvent) -> SourceEvent {
    let now = Utc::now();
    let event_lower = hook.event.to_lowercase();

    // Resolve provider from the optional field, defaulting to Claude.
    let provider = match hook.provider.as_deref() {
        Some("codex") => Provider::Codex,
        Some("gemini") => Provider::Gemini,
        Some("copilot") => Provider::Copilot,
        Some("claude") | None => Provider::Claude,
        Some(_) => Provider::Claude, // fallback for unknown providers
    };

    // Infer activity state, weight, and confidence from event type.
    let (signal, weight, confidence) = match event_lower.as_str() {
        // Tool use events indicate the agent is actively running.
        "pretooluse" | "pre_tool_use" => (ActivityState::Running, 0.90, 0.85),
        "posttooluse" | "post_tool_use" => (ActivityState::Running, 0.85, 0.80),

        // Stop events indicate the agent has become idle.
        "stop" | "stopped" | "done" => (ActivityState::Idle, 0.90, 0.90),

        // Approval/confirmation events.
        "approval" | "waiting_approval" | "needsapproval" => {
            (ActivityState::WaitingApproval, 0.95, 0.90)
        }

        // Input-waiting events.
        "waiting_input" | "needsinput" | "prompt" => {
            (ActivityState::WaitingInput, 0.90, 0.85)
        }

        // Error events.
        "error" | "failed" => (ActivityState::Error, 0.90, 0.85),

        // Unknown events still get recorded as raw signals.
        _ => (ActivityState::Unknown, 0.50, 0.50),
    };

    let evidence = Evidence {
        provider,
        kind: EvidenceKind::HookEvent(hook.event.clone()),
        signal,
        weight,
        confidence,
        timestamp: now,
        ttl: Duration::from_secs(120),
        source: SourceType::Hook,
        reason_code: format!("hook:{}", hook.event),
    };

    SourceEvent::Evidence {
        pane_id: hook.pane_id.clone(),
        evidence: vec![evidence],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hook(event: &str, pane_id: &str) -> HookEvent {
        HookEvent {
            event: event.to_string(),
            pane_id: pane_id.to_string(),
            provider: None,
            payload: serde_json::Value::Null,
        }
    }

    fn make_hook_with_provider(event: &str, pane_id: &str, provider: Option<&str>) -> HookEvent {
        HookEvent {
            event: event.to_string(),
            pane_id: pane_id.to_string(),
            provider: provider.map(|s| s.to_string()),
            payload: serde_json::Value::Null,
        }
    }

    /// Extract the first Evidence from a SourceEvent::Evidence variant.
    fn extract_evidence(event: SourceEvent) -> Evidence {
        match event {
            SourceEvent::Evidence { evidence, .. } => {
                assert!(!evidence.is_empty(), "expected at least one evidence");
                evidence.into_iter().next().unwrap()
            }
            other => panic!("expected SourceEvent::Evidence, got: {other:?}"),
        }
    }

    #[test]
    fn pre_tool_use_maps_to_running() {
        let hook = make_hook("PreToolUse", "%1");
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(matches!(ev.signal, ActivityState::Running));
        assert_eq!(ev.weight, 0.90);
        assert_eq!(ev.confidence, 0.85);
        assert_eq!(ev.reason_code, "hook:PreToolUse");
    }

    #[test]
    fn post_tool_use_maps_to_running() {
        let hook = make_hook("PostToolUse", "%2");
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(matches!(ev.signal, ActivityState::Running));
        assert_eq!(ev.weight, 0.85);
        assert_eq!(ev.confidence, 0.80);
    }

    #[test]
    fn stop_maps_to_idle() {
        for event_name in &["stop", "stopped", "done"] {
            let hook = make_hook(event_name, "%1");
            let ev = extract_evidence(hook_to_source_event(&hook));
            assert!(
                matches!(ev.signal, ActivityState::Idle),
                "expected Idle for event '{event_name}', got {:?}",
                ev.signal,
            );
            assert_eq!(ev.weight, 0.90);
            assert_eq!(ev.confidence, 0.90);
        }
    }

    #[test]
    fn approval_maps_to_waiting_approval() {
        for event_name in &["approval", "waiting_approval", "NeedsApproval"] {
            let hook = make_hook(event_name, "%3");
            let ev = extract_evidence(hook_to_source_event(&hook));
            assert!(
                matches!(ev.signal, ActivityState::WaitingApproval),
                "expected WaitingApproval for event '{event_name}', got {:?}",
                ev.signal,
            );
            assert_eq!(ev.weight, 0.95);
            assert_eq!(ev.confidence, 0.90);
        }
    }

    #[test]
    fn error_maps_to_error() {
        for event_name in &["error", "failed"] {
            let hook = make_hook(event_name, "%1");
            let ev = extract_evidence(hook_to_source_event(&hook));
            assert!(
                matches!(ev.signal, ActivityState::Error),
                "expected Error for event '{event_name}', got {:?}",
                ev.signal,
            );
            assert_eq!(ev.weight, 0.90);
            assert_eq!(ev.confidence, 0.85);
        }
    }

    #[test]
    fn unknown_event_maps_to_unknown() {
        let hook = make_hook("SomeRandomEvent", "%5");
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(matches!(ev.signal, ActivityState::Unknown));
        assert_eq!(ev.weight, 0.50);
        assert_eq!(ev.confidence, 0.50);
        assert_eq!(ev.reason_code, "hook:SomeRandomEvent");
    }

    #[test]
    fn waiting_input_maps_correctly() {
        for event_name in &["waiting_input", "NeedsInput", "prompt"] {
            let hook = make_hook(event_name, "%4");
            let ev = extract_evidence(hook_to_source_event(&hook));
            assert!(
                matches!(ev.signal, ActivityState::WaitingInput),
                "expected WaitingInput for event '{event_name}', got {:?}",
                ev.signal,
            );
            assert_eq!(ev.weight, 0.90);
            assert_eq!(ev.confidence, 0.85);
        }
    }

    #[test]
    fn pane_id_preserved_in_event() {
        let hook = make_hook("PreToolUse", "%42");
        let event = hook_to_source_event(&hook);
        match event {
            SourceEvent::Evidence { pane_id, .. } => {
                assert_eq!(pane_id, "%42");
            }
            other => panic!("expected Evidence, got: {other:?}"),
        }
    }

    #[test]
    fn evidence_fields_are_set_correctly() {
        let hook = make_hook("PreToolUse", "%1");
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(matches!(ev.provider, Provider::Claude));
        assert!(matches!(ev.kind, EvidenceKind::HookEvent(ref s) if s == "PreToolUse"));
        assert!(matches!(ev.source, SourceType::Hook));
        assert_eq!(ev.ttl, Duration::from_secs(120));
    }

    // -----------------------------------------------------------------------
    // Provider field handling tests
    // -----------------------------------------------------------------------

    #[test]
    fn hook_with_codex_provider() {
        let hook = make_hook_with_provider("PreToolUse", "%1", Some("codex"));
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(
            matches!(ev.provider, Provider::Codex),
            "expected Codex, got {:?}",
            ev.provider,
        );
    }

    #[test]
    fn hook_with_no_provider_defaults_to_claude() {
        let hook = make_hook_with_provider("PreToolUse", "%1", None);
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(
            matches!(ev.provider, Provider::Claude),
            "expected Claude (default), got {:?}",
            ev.provider,
        );
    }

    #[test]
    fn hook_with_unknown_provider_defaults_to_claude() {
        let hook = make_hook_with_provider("PreToolUse", "%1", Some("unknown_provider"));
        let ev = extract_evidence(hook_to_source_event(&hook));
        assert!(
            matches!(ev.provider, Provider::Claude),
            "expected Claude (fallback), got {:?}",
            ev.provider,
        );
    }
}
