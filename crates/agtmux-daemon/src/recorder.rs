use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::orchestrator::StateNotification;

/// A single recorded event line in the JSONL file.
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordedEvent {
    /// Wall-clock timestamp when the event was recorded.
    pub ts: String,
    /// The notification that was recorded.
    #[serde(flatten)]
    pub event: StateNotification,
}

pub struct Recorder {
    writer: std::fs::File,
    rx: broadcast::Receiver<StateNotification>,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
}

impl Recorder {
    pub fn new(
        path: &Path,
        rx: broadcast::Receiver<StateNotification>,
    ) -> std::io::Result<Self> {
        Self::with_cancel(path, rx, CancellationToken::new())
    }

    /// Create a recorder with an explicit cancellation token for graceful shutdown.
    pub fn with_cancel(
        path: &Path,
        rx: broadcast::Receiver<StateNotification>,
        cancel: CancellationToken,
    ) -> std::io::Result<Self> {
        let writer = std::fs::File::create(path)?;
        Ok(Self { writer, rx, cancel })
    }

    /// Run the recorder. Writes one JSON line per event until cancelled or
    /// the broadcast channel is closed.
    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                result = self.rx.recv() => {
                    match result {
                        Ok(notification) => {
                            let record = RecordedEvent {
                                ts: Utc::now().to_rfc3339(),
                                event: notification,
                            };
                            match serde_json::to_string(&record) {
                                Ok(line) => {
                                    if let Err(e) = writeln!(self.writer, "{}", line) {
                                        tracing::error!("recorder write failed: {e}");
                                    }
                                    if let Err(e) = self.writer.flush() {
                                        tracing::error!("recorder flush failed: {e}");
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("recorder serialization failed: {e}");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "recorder lagged, dropped events");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::info!("recorder: broadcast channel closed, stopping");
                            break;
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("recorder: cancellation requested, shutting down");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::PaneState;
    use agtmux_core::engine::ResolvedActivity;
    use agtmux_core::types::*;
    use chrono::Utc;

    fn sample_pane_state() -> PaneState {
        PaneState {
            pane_id: "%1".into(),
            provider: Some(Provider::Claude),
            provider_confidence: 0.95,
            activity: ResolvedActivity {
                state: ActivityState::Running,
                confidence: 0.9,
                source: SourceType::Hook,
                reason_code: "running".into(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: String::new(),
                since: None,
            },
            last_event_type: "evidence".into(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn recorded_event_serialization_contains_ts_and_type() {
        let event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::StateChanged {
                pane_id: "%1".into(),
                state: sample_pane_state(),
            },
        };

        let json = serde_json::to_string(&event).expect("should serialize");

        // Verify ts field is present
        assert!(json.contains("\"ts\":\"2026-02-23T12:00:00+00:00\""));
        // Verify the flattened event fields are present
        assert!(json.contains("\"type\":\"StateChanged\""));
        assert!(json.contains("\"pane_id\":\"%1\""));
    }

    #[test]
    fn recorded_event_pane_added_serialization() {
        let event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::PaneAdded {
                pane_id: "%42".into(),
            },
        };

        let json = serde_json::to_string(&event).expect("should serialize");
        assert!(json.contains("\"type\":\"PaneAdded\""));
        assert!(json.contains("\"%42\""));
    }

    #[test]
    fn recorded_event_pane_removed_serialization() {
        let event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::PaneRemoved {
                pane_id: "%99".into(),
            },
        };

        let json = serde_json::to_string(&event).expect("should serialize");
        assert!(json.contains("\"type\":\"PaneRemoved\""));
        assert!(json.contains("\"%99\""));
    }

    #[test]
    fn state_notification_round_trip_state_changed() {
        let notification = StateNotification::StateChanged {
            pane_id: "%1".into(),
            state: sample_pane_state(),
        };

        let json = serde_json::to_string(&notification).expect("should serialize");
        let deserialized: StateNotification =
            serde_json::from_str(&json).expect("should deserialize");

        match deserialized {
            StateNotification::StateChanged { pane_id, state } => {
                assert_eq!(pane_id, "%1");
                assert_eq!(state.activity.state, ActivityState::Running);
                assert_eq!(state.provider, Some(Provider::Claude));
                assert!((state.provider_confidence - 0.95).abs() < f64::EPSILON);
            }
            other => panic!("expected StateChanged, got {:?}", other),
        }
    }

    #[test]
    fn state_notification_round_trip_pane_added() {
        let notification = StateNotification::PaneAdded {
            pane_id: "%5".into(),
        };

        let json = serde_json::to_string(&notification).expect("should serialize");
        let deserialized: StateNotification =
            serde_json::from_str(&json).expect("should deserialize");

        match deserialized {
            StateNotification::PaneAdded { pane_id } => assert_eq!(pane_id, "%5"),
            other => panic!("expected PaneAdded, got {:?}", other),
        }
    }

    #[test]
    fn state_notification_round_trip_pane_removed() {
        let notification = StateNotification::PaneRemoved {
            pane_id: "%7".into(),
        };

        let json = serde_json::to_string(&notification).expect("should serialize");
        let deserialized: StateNotification =
            serde_json::from_str(&json).expect("should deserialize");

        match deserialized {
            StateNotification::PaneRemoved { pane_id } => assert_eq!(pane_id, "%7"),
            other => panic!("expected PaneRemoved, got {:?}", other),
        }
    }

    #[test]
    fn recorded_event_round_trip() {
        let event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::StateChanged {
                pane_id: "%1".into(),
                state: sample_pane_state(),
            },
        };

        let json = serde_json::to_string(&event).expect("should serialize");
        let deserialized: RecordedEvent =
            serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.ts, "2026-02-23T12:00:00+00:00");
        match deserialized.event {
            StateNotification::StateChanged { pane_id, state } => {
                assert_eq!(pane_id, "%1");
                assert_eq!(state.activity.state, ActivityState::Running);
            }
            other => panic!("expected StateChanged, got {:?}", other),
        }
    }

    /// Cross-module integration test: recorder → label → accuracy pipeline.
    ///
    /// 1. Creates a `recorder::RecordedEvent` from a `StateNotification::StateChanged`.
    /// 2. Serializes it to JSON.
    /// 3. Deserializes it as `label::RecordedEvent`.
    /// 4. Verifies that `accuracy::extract_predicted_state` correctly extracts the
    ///    predicted state.
    /// 5. Verifies that adding a label and re-serializing preserves all fields.
    #[test]
    fn cross_module_recorder_label_accuracy_pipeline() {
        use crate::accuracy::extract_predicted_state;
        use crate::label::RecordedEvent as LabelRecordedEvent;

        // Step 1: Create a recorder::RecordedEvent from a StateNotification::StateChanged.
        let recorder_event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::StateChanged {
                pane_id: "%1".into(),
                state: sample_pane_state(),
            },
        };

        // Step 2: Serialize to JSON.
        let json = serde_json::to_string(&recorder_event).expect("recorder event should serialize");

        // Step 3: Deserialize as label::RecordedEvent.
        let label_event: LabelRecordedEvent =
            serde_json::from_str(&json).expect("should deserialize as label::RecordedEvent");
        assert_eq!(label_event.ts, "2026-02-23T12:00:00+00:00");
        assert_eq!(label_event.event_type, "StateChanged");
        assert!(label_event.label.is_none());

        // Step 4: Verify extract_predicted_state correctly extracts the predicted state.
        let predicted = extract_predicted_state(&label_event.data);
        assert_eq!(predicted, Some("running".to_string()));

        // Step 5: Add a label and re-serialize, then verify all fields are preserved.
        let mut labeled_event = label_event.clone();
        labeled_event.label = Some("running".to_string());

        let labeled_json = serde_json::to_string(&labeled_event).expect("labeled event should serialize");

        // Re-deserialize and check everything is still present.
        let round_tripped: LabelRecordedEvent =
            serde_json::from_str(&labeled_json).expect("labeled JSON should round-trip");
        assert_eq!(round_tripped.ts, "2026-02-23T12:00:00+00:00");
        assert_eq!(round_tripped.event_type, "StateChanged");
        assert_eq!(round_tripped.label, Some("running".to_string()));

        // The predicted state should still be extractable after round-trip.
        let predicted_after = extract_predicted_state(&round_tripped.data);
        assert_eq!(predicted_after, Some("running".to_string()));

        // Verify the pane_id is preserved in the data field.
        let pane_id = round_tripped.data.get("pane_id").and_then(|v| v.as_str());
        assert_eq!(pane_id, Some("%1"));
    }

    #[test]
    fn jsonl_line_is_single_line() {
        let event = RecordedEvent {
            ts: "2026-02-23T12:00:00+00:00".into(),
            event: StateNotification::StateChanged {
                pane_id: "%1".into(),
                state: sample_pane_state(),
            },
        };

        let json = serde_json::to_string(&event).expect("should serialize");
        // A JSONL line must not contain newlines
        assert!(!json.contains('\n'));
        assert!(!json.contains('\r'));
    }
}
