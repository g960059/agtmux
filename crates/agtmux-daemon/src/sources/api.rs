use std::time::Duration;

use agtmux_core::source::SourceEvent;
use agtmux_core::types::{
    ActivityState, Evidence, EvidenceKind, Provider, SourceType,
};
use chrono::Utc;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Connects to the Codex CLI app-server WebSocket and converts real-time
/// status messages into `SourceEvent`s for the orchestrator pipeline.
pub struct ApiSource {
    tx: mpsc::Sender<SourceEvent>,
    url: String,
    pane_id: String,
    cancel: CancellationToken,
}

impl ApiSource {
    pub fn new(
        tx: mpsc::Sender<SourceEvent>,
        url: String,
        pane_id: String,
        cancel: CancellationToken,
    ) -> Self {
        Self { tx, url, pane_id, cancel }
    }

    /// Connect to the Codex app-server WebSocket and listen for status
    /// messages. Automatically retries on disconnection with exponential
    /// backoff (3s, 6s, 12s, 24s, 48s, capped at 60s). Resets backoff on
    /// successful connection. After 10 consecutive failures, logs at debug
    /// level instead of warn.
    ///
    /// Blocks until cancelled via the `CancellationToken`.
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const INITIAL_BACKOFF_SECS: u64 = 3;
        const MAX_BACKOFF_SECS: u64 = 60;
        const DEBUG_LOG_THRESHOLD: u32 = 10;

        let mut backoff_secs = INITIAL_BACKOFF_SECS;
        let mut consecutive_failures: u32 = 0;

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    tracing::info!("api source: cancellation requested, shutting down");
                    return Ok(());
                }
                result = self.connect_and_listen() => {
                    match result {
                        Ok(()) => {
                            tracing::info!("api source: connection closed cleanly");
                            // Reset backoff on successful connection.
                            backoff_secs = INITIAL_BACKOFF_SECS;
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            if consecutive_failures >= DEBUG_LOG_THRESHOLD {
                                tracing::debug!(
                                    consecutive_failures,
                                    "api source: connection error: {e}"
                                );
                            } else {
                                tracing::warn!("api source: connection error: {e}");
                            }
                        }
                    }
                }
            }

            // Wait before retrying, but bail if cancelled.
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    tracing::info!("api source: cancellation during retry backoff");
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {
                    tracing::info!(
                        url = %self.url,
                        backoff_secs,
                        consecutive_failures,
                        "api source: reconnecting..."
                    );
                }
            }

            // Exponential backoff: double the delay, capped at MAX_BACKOFF_SECS.
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }
    }

    /// Single connection attempt: connect, read messages until EOF or error.
    async fn connect_and_listen(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&self.url).await?;
        tracing::info!(url = %self.url, "api source: connected to codex app-server");

        let (_write, mut read) = ws_stream.split();

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    return Ok(());
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(message)) => {
                            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                                if let Some(state) = parse_codex_message(&text) {
                                    let evidence = build_api_evidence(state, &self.pane_id);
                                    let event = SourceEvent::Evidence {
                                        pane_id: self.pane_id.clone(),
                                        evidence,
                                        meta: None,
                                    };
                                    if let Err(e) = self.tx.send(event).await {
                                        tracing::warn!("api source: failed to send event: {e}");
                                        return Ok(());
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(Box::new(e));
                        }
                        None => {
                            // Stream ended.
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// Parse a Codex app-server WebSocket message into an `ActivityState`.
///
/// Expected message formats:
/// ```json
/// {"type": "status", "status": "running"}
/// {"type": "status", "status": "idle"}
/// {"type": "status", "status": "waiting_for_approval"}
/// {"type": "error", "message": "..."}
/// ```
pub fn parse_codex_message(msg: &str) -> Option<ActivityState> {
    let value: serde_json::Value = serde_json::from_str(msg).ok()?;
    let msg_type = value.get("type")?.as_str()?;

    match msg_type {
        "status" => {
            let status = value.get("status")?.as_str()?;
            match status {
                "running" => Some(ActivityState::Running),
                "idle" => Some(ActivityState::Idle),
                "waiting_for_approval" => Some(ActivityState::WaitingApproval),
                _ => None,
            }
        }
        "error" => Some(ActivityState::Error),
        _ => None,
    }
}

/// Build evidence from an API-sourced activity state.
pub fn build_api_evidence(state: ActivityState, pane_id: &str) -> Vec<Evidence> {
    let now = Utc::now();

    let (weight, confidence) = match state {
        ActivityState::Running => (0.90, 0.90),
        ActivityState::Idle => (0.85, 0.85),
        ActivityState::WaitingApproval => (0.95, 0.90),
        ActivityState::Error => (0.90, 0.85),
        ActivityState::WaitingInput => (0.90, 0.85),
        ActivityState::Unknown => (0.50, 0.50),
        // non_exhaustive: new variants added to ActivityState will hit this arm.
        // Use low confidence so the engine does not over-trust an unrecognized state.
        _ => (0.50, 0.50),
    };

    let evidence = Evidence {
        provider: Provider::Codex,
        kind: EvidenceKind::ApiNotification(format!("codex:{state:?}")),
        signal: state,
        weight,
        confidence,
        timestamp: now,
        ttl: Duration::from_secs(90),
        source: SourceType::Api,
        reason_code: format!("api:codex:{}", pane_id),
    };

    vec![evidence]
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_codex_message tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_running_status() {
        let msg = r#"{"type": "status", "status": "running"}"#;
        assert_eq!(parse_codex_message(msg), Some(ActivityState::Running));
    }

    #[test]
    fn parse_idle_status() {
        let msg = r#"{"type": "status", "status": "idle"}"#;
        assert_eq!(parse_codex_message(msg), Some(ActivityState::Idle));
    }

    #[test]
    fn parse_waiting_for_approval_status() {
        let msg = r#"{"type": "status", "status": "waiting_for_approval"}"#;
        assert_eq!(
            parse_codex_message(msg),
            Some(ActivityState::WaitingApproval)
        );
    }

    #[test]
    fn parse_error_message() {
        let msg = r#"{"type": "error", "message": "rate limited"}"#;
        assert_eq!(parse_codex_message(msg), Some(ActivityState::Error));
    }

    #[test]
    fn parse_unknown_status_returns_none() {
        let msg = r#"{"type": "status", "status": "something_unknown"}"#;
        assert_eq!(parse_codex_message(msg), None);
    }

    #[test]
    fn parse_unknown_type_returns_none() {
        let msg = r#"{"type": "heartbeat", "ts": 12345}"#;
        assert_eq!(parse_codex_message(msg), None);
    }

    #[test]
    fn parse_malformed_json_returns_none() {
        let msg = "this is not valid json {{{";
        assert_eq!(parse_codex_message(msg), None);
    }

    #[test]
    fn parse_missing_type_field_returns_none() {
        let msg = r#"{"status": "running"}"#;
        assert_eq!(parse_codex_message(msg), None);
    }

    #[test]
    fn parse_status_missing_status_field_returns_none() {
        let msg = r#"{"type": "status"}"#;
        assert_eq!(parse_codex_message(msg), None);
    }

    // -----------------------------------------------------------------------
    // build_api_evidence tests
    // -----------------------------------------------------------------------

    #[test]
    fn evidence_has_correct_provider() {
        let evidence = build_api_evidence(ActivityState::Running, "%1");
        assert_eq!(evidence.len(), 1);
        assert!(
            matches!(evidence[0].provider, Provider::Codex),
            "expected Provider::Codex, got {:?}",
            evidence[0].provider,
        );
    }

    #[test]
    fn evidence_uses_source_type_api() {
        let evidence = build_api_evidence(ActivityState::Idle, "%1");
        assert!(
            matches!(evidence[0].source, SourceType::Api),
            "expected SourceType::Api, got {:?}",
            evidence[0].source,
        );
    }

    #[test]
    fn evidence_kind_is_api_notification() {
        let evidence = build_api_evidence(ActivityState::Running, "%1");
        assert!(
            matches!(evidence[0].kind, EvidenceKind::ApiNotification(_)),
            "expected EvidenceKind::ApiNotification, got {:?}",
            evidence[0].kind,
        );
    }

    #[test]
    fn evidence_signal_matches_input_state() {
        for state in [
            ActivityState::Running,
            ActivityState::Idle,
            ActivityState::WaitingApproval,
            ActivityState::Error,
        ] {
            let evidence = build_api_evidence(state, "%1");
            assert_eq!(
                evidence[0].signal, state,
                "signal should match input state {state:?}"
            );
        }
    }

    #[test]
    fn evidence_ttl_is_90_seconds() {
        let evidence = build_api_evidence(ActivityState::Running, "%1");
        assert_eq!(evidence[0].ttl, Duration::from_secs(90));
    }

    #[test]
    fn evidence_reason_code_contains_pane_id() {
        let evidence = build_api_evidence(ActivityState::Running, "%42");
        assert!(
            evidence[0].reason_code.starts_with("api:codex:"),
            "reason_code should start with 'api:codex:', got: {}",
            evidence[0].reason_code,
        );
        assert!(
            evidence[0].reason_code.contains("%42"),
            "reason_code should contain pane_id, got: {}",
            evidence[0].reason_code,
        );
    }

    #[test]
    fn evidence_weight_and_confidence_for_running() {
        let evidence = build_api_evidence(ActivityState::Running, "%1");
        assert_eq!(evidence[0].weight, 0.90);
        assert_eq!(evidence[0].confidence, 0.90);
    }

    #[test]
    fn evidence_weight_and_confidence_for_waiting_approval() {
        let evidence = build_api_evidence(ActivityState::WaitingApproval, "%1");
        assert_eq!(evidence[0].weight, 0.95);
        assert_eq!(evidence[0].confidence, 0.90);
    }

    #[test]
    fn evidence_weight_and_confidence_for_unknown() {
        let evidence = build_api_evidence(ActivityState::Unknown, "%1");
        assert_eq!(evidence[0].weight, 0.50);
        assert_eq!(evidence[0].confidence, 0.50);
    }
}
