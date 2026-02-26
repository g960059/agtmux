//! Poller accuracy evaluator: runs detection + activity against a labeled
//! fixture dataset and computes weighted F1, per-class metrics, and
//! waiting recall.
//!
//! Quality gate: `weighted F1 >= 0.85` AND `waiting recall >= 0.85`.
//!
//! Task ref: T-033

use agtmux_core_v5::types::{ActivityState, Provider};
use serde::{Deserialize, Serialize};

use crate::source::{PaneSnapshot, poll_pane};

// ─── Fixture schema ─────────────────────────────────────────────

/// A single labeled window in the fixture dataset.
#[derive(Debug, Clone, Deserialize)]
pub struct LabeledWindow {
    pub pane_id: String,
    pub pane_title: String,
    pub current_cmd: String,
    pub process_hint: Option<String>,
    pub capture_lines: Vec<String>,
    /// Whether an agent should be detected.
    pub expected_detected: bool,
    /// Expected provider if detected.
    pub expected_provider: Option<String>,
    /// Expected activity state if detected.
    pub expected_activity: Option<String>,
}

impl LabeledWindow {
    /// Convert to a `PaneSnapshot` for evaluation.
    pub fn to_snapshot(&self) -> PaneSnapshot {
        PaneSnapshot {
            pane_id: self.pane_id.clone(),
            pane_title: self.pane_title.clone(),
            current_cmd: self.current_cmd.clone(),
            process_hint: self.process_hint.clone(),
            capture_lines: self.capture_lines.clone(),
            captured_at: chrono::Utc::now(),
        }
    }

    /// Parse expected provider. Returns `Err` on unknown values to catch fixture typos.
    pub fn expected_provider_enum(&self) -> Result<Option<Provider>, String> {
        match self.expected_provider.as_deref() {
            None => Ok(None),
            Some("claude") => Ok(Some(Provider::Claude)),
            Some("codex") => Ok(Some(Provider::Codex)),
            Some(other) => Err(format!(
                "invalid expected_provider '{}' in pane {}",
                other, self.pane_id
            )),
        }
    }

    /// Parse expected activity state. Returns `Err` on unknown values to catch fixture typos.
    pub fn expected_activity_enum(&self) -> Result<Option<ActivityState>, String> {
        match self.expected_activity.as_deref() {
            None => Ok(None),
            Some("Running") => Ok(Some(ActivityState::Running)),
            Some("Idle") => Ok(Some(ActivityState::Idle)),
            Some("WaitingApproval") => Ok(Some(ActivityState::WaitingApproval)),
            Some("Error") => Ok(Some(ActivityState::Error)),
            Some("Unknown") => Ok(Some(ActivityState::Unknown)),
            Some(other) => Err(format!(
                "invalid expected_activity '{}' in pane {}",
                other, self.pane_id
            )),
        }
    }
}

// ─── Evaluation result ──────────────────────────────────────────

/// Per-class precision/recall/F1 metrics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClassMetrics {
    pub true_positives: u32,
    pub false_positives: u32,
    pub false_negatives: u32,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub support: u32,
}

impl ClassMetrics {
    fn compute(&mut self) {
        let tp = self.true_positives as f64;
        let fp = self.false_positives as f64;
        let r#fn = self.false_negatives as f64;

        self.precision = if tp + fp > 0.0 { tp / (tp + fp) } else { 0.0 };
        self.recall = if tp + r#fn > 0.0 {
            tp / (tp + r#fn)
        } else {
            0.0
        };
        self.f1 = if self.precision + self.recall > 0.0 {
            2.0 * self.precision * self.recall / (self.precision + self.recall)
        } else {
            0.0
        };
    }
}

/// Full evaluation report.
#[derive(Debug, Clone, Serialize)]
pub struct EvaluationReport {
    /// Total number of windows evaluated.
    pub total_windows: usize,
    /// Detection accuracy (correct detected/not-detected).
    pub detection_accuracy: f64,
    /// Provider accuracy among detected windows (correct provider / total detected).
    pub provider_accuracy: f64,
    /// Per-activity-state metrics.
    pub activity_metrics: Vec<(String, ClassMetrics)>,
    /// Weighted F1 across activity states (weighted by support).
    pub weighted_f1: f64,
    /// Recall specifically for WaitingApproval class.
    pub waiting_recall: f64,
    /// Gate pass/fail.
    pub gate_pass: bool,
    /// Individual gate results.
    pub weighted_f1_pass: bool,
    pub waiting_recall_pass: bool,
}

// ─── Gate thresholds ────────────────────────────────────────────

/// Minimum weighted F1 for gate pass (FR-032).
pub const GATE_WEIGHTED_F1: f64 = 0.85;
/// Minimum WaitingApproval recall for gate pass (FR-032).
pub const GATE_WAITING_RECALL: f64 = 0.85;

// ─── Evaluator ──────────────────────────────────────────────────

/// Evaluate the poller against a labeled fixture dataset.
///
/// Returns a detailed report with per-class metrics and gate verdict.
pub fn evaluate(fixtures: &[LabeledWindow]) -> EvaluationReport {
    // Activity state labels we track.
    let labels = [
        ("Running", ActivityState::Running),
        ("Idle", ActivityState::Idle),
        ("WaitingApproval", ActivityState::WaitingApproval),
        ("Error", ActivityState::Error),
        ("Unknown", ActivityState::Unknown),
    ];

    let mut per_class: Vec<(String, ClassMetrics)> = labels
        .iter()
        .map(|(name, _)| (name.to_string(), ClassMetrics::default()))
        .collect();

    let mut detection_correct = 0usize;
    let mut provider_correct = 0usize;
    let mut provider_total = 0usize;
    let total = fixtures.len();

    for window in fixtures {
        let snapshot = window.to_snapshot();
        let result = poll_pane(&snapshot);

        let actual_detected = result.is_some();

        // Detection accuracy
        if actual_detected == window.expected_detected {
            detection_correct += 1;
        }

        // Provider accuracy (only for expected-detected + actually-detected windows)
        if window.expected_detected && actual_detected {
            provider_total += 1;
            let expected_provider = window
                .expected_provider_enum()
                .expect("fixture label validation failed")
                .unwrap_or(Provider::Claude);
            let actual_provider = result.as_ref().map(|r| r.provider);
            if actual_provider == Some(expected_provider) {
                provider_correct += 1;
            }
        }

        // Activity classification (only for detected + expected-detected windows)
        if window.expected_detected {
            let expected_activity = window
                .expected_activity_enum()
                .expect("fixture label validation failed")
                .unwrap_or(ActivityState::Unknown);
            let actual_activity = result
                .as_ref()
                .map_or(ActivityState::Unknown, |r| r.activity_state);

            for (i, (_, label_state)) in labels.iter().enumerate() {
                let is_expected = expected_activity == *label_state;
                let is_actual = actual_activity == *label_state;

                if is_expected {
                    per_class[i].1.support += 1;
                }
                match (is_actual, is_expected) {
                    (true, true) => per_class[i].1.true_positives += 1,
                    (true, false) => per_class[i].1.false_positives += 1,
                    (false, true) => per_class[i].1.false_negatives += 1,
                    (false, false) => {} // true negative
                }
            }
        }
    }

    // Compute per-class metrics
    for (_, metrics) in &mut per_class {
        metrics.compute();
    }

    let detection_accuracy = if total > 0 {
        detection_correct as f64 / total as f64
    } else {
        0.0
    };

    let provider_accuracy = if provider_total > 0 {
        provider_correct as f64 / provider_total as f64
    } else {
        0.0
    };

    // Weighted F1 (weighted by support)
    let total_support: u32 = per_class.iter().map(|(_, m)| m.support).sum();
    let weighted_f1 = if total_support > 0 {
        per_class
            .iter()
            .map(|(_, m)| m.f1 * m.support as f64)
            .sum::<f64>()
            / total_support as f64
    } else {
        0.0
    };

    // WaitingApproval recall
    let waiting_recall = per_class
        .iter()
        .find(|(name, _)| name == "WaitingApproval")
        .map_or(0.0, |(_, m)| m.recall);

    // Gate verdict
    let weighted_f1_pass = weighted_f1 >= GATE_WEIGHTED_F1;
    let waiting_recall_pass = waiting_recall >= GATE_WAITING_RECALL;
    let gate_pass = weighted_f1_pass && waiting_recall_pass;

    EvaluationReport {
        total_windows: total,
        detection_accuracy,
        provider_accuracy,
        activity_metrics: per_class,
        weighted_f1,
        waiting_recall,
        gate_pass,
        weighted_f1_pass,
        waiting_recall_pass,
    }
}

/// Load fixtures from a JSON file.
pub fn load_fixtures(path: &str) -> Result<Vec<LabeledWindow>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read fixture file {path}: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse fixture JSON: {e}"))
}

/// Pretty-print an evaluation report to stdout.
pub fn print_report(report: &EvaluationReport) {
    println!("=== Poller Baseline Quality Gate ===");
    println!();
    println!("Total windows: {}", report.total_windows);
    println!(
        "Detection accuracy: {:.1}%",
        report.detection_accuracy * 100.0
    );
    println!(
        "Provider accuracy:  {:.1}%",
        report.provider_accuracy * 100.0
    );
    println!();
    println!(
        "{:<20} {:>5} {:>8} {:>8} {:>8}",
        "Class", "N", "Prec", "Recall", "F1"
    );
    println!("{}", "-".repeat(54));
    for (name, metrics) in &report.activity_metrics {
        println!(
            "{:<20} {:>5} {:>7.1}% {:>7.1}% {:>7.1}%",
            name,
            metrics.support,
            metrics.precision * 100.0,
            metrics.recall * 100.0,
            metrics.f1 * 100.0,
        );
    }
    println!("{}", "-".repeat(54));
    println!();
    println!(
        "Weighted F1:       {:.1}% (gate: >= {:.0}%) {}",
        report.weighted_f1 * 100.0,
        GATE_WEIGHTED_F1 * 100.0,
        if report.weighted_f1_pass {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!(
        "Waiting recall:    {:.1}% (gate: >= {:.0}%) {}",
        report.waiting_recall * 100.0,
        GATE_WAITING_RECALL * 100.0,
        if report.waiting_recall_pass {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!();
    println!(
        "Gate verdict: {}",
        if report.gate_pass { "PASS" } else { "FAIL" }
    );
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_window(
        pane_title: &str,
        current_cmd: &str,
        process_hint: Option<&str>,
        capture_lines: &[&str],
        expected_detected: bool,
        expected_provider: Option<&str>,
        expected_activity: Option<&str>,
    ) -> LabeledWindow {
        LabeledWindow {
            pane_id: "%test".to_string(),
            pane_title: pane_title.to_string(),
            current_cmd: current_cmd.to_string(),
            process_hint: process_hint.map(String::from),
            capture_lines: capture_lines.iter().map(|s| s.to_string()).collect(),
            expected_detected,
            expected_provider: expected_provider.map(String::from),
            expected_activity: expected_activity.map(String::from),
        }
    }

    #[test]
    fn evaluate_empty_dataset() {
        let report = evaluate(&[]);
        assert_eq!(report.total_windows, 0);
        assert!(!report.gate_pass);
    }

    #[test]
    fn evaluate_perfect_detection() {
        let fixtures = vec![
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Thinking"],
                true,
                Some("claude"),
                Some("Running"),
            ),
            make_window(
                "codex",
                "codex",
                Some("codex"),
                &["Processing"],
                true,
                Some("codex"),
                Some("Running"),
            ),
            make_window("vim", "bash", None, &["normal text"], false, None, None),
        ];
        let report = evaluate(&fixtures);
        assert_eq!(report.total_windows, 3);
        assert!((report.detection_accuracy - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_claude_running_correct() {
        let fixtures = vec![make_window(
            "claude code",
            "claude",
            Some("claude"),
            &["Thinking about it"],
            true,
            Some("claude"),
            Some("Running"),
        )];
        let report = evaluate(&fixtures);
        let running = report.activity_metrics.iter().find(|(n, _)| n == "Running");
        assert!(running.is_some());
        let (_, m) = running.expect("Running class should exist");
        assert_eq!(m.true_positives, 1);
        assert_eq!(m.false_positives, 0);
        assert_eq!(m.false_negatives, 0);
        assert!((m.f1 - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_waiting_recall() {
        let fixtures = vec![
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Allow? press Y"],
                true,
                Some("claude"),
                Some("WaitingApproval"),
            ),
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["approve this change"],
                true,
                Some("claude"),
                Some("WaitingApproval"),
            ),
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["permission needed"],
                true,
                Some("claude"),
                Some("WaitingApproval"),
            ),
        ];
        let report = evaluate(&fixtures);
        assert!((report.waiting_recall - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_weighted_f1_calculation() {
        // All correct predictions
        let fixtures = vec![
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Thinking"],
                true,
                Some("claude"),
                Some("Running"),
            ),
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Thinking"],
                true,
                Some("claude"),
                Some("Running"),
            ),
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["\u{276f}"],
                true,
                Some("claude"),
                Some("Idle"),
            ),
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Allow?"],
                true,
                Some("claude"),
                Some("WaitingApproval"),
            ),
        ];
        let report = evaluate(&fixtures);
        // All correct → weighted F1 should be 1.0
        assert!((report.weighted_f1 - 1.0).abs() < f64::EPSILON);
        assert!(report.gate_pass);
    }

    #[test]
    fn evaluate_no_agent_windows() {
        let fixtures = vec![
            make_window("vim", "bash", None, &["editing file"], false, None, None),
            make_window("htop", "htop", None, &["CPU 5%"], false, None, None),
        ];
        let report = evaluate(&fixtures);
        assert_eq!(report.total_windows, 2);
        assert!((report.detection_accuracy - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_misclassified_activity() {
        // Label says WaitingApproval but capture shows Running pattern
        let fixtures = vec![make_window(
            "claude code",
            "claude",
            Some("claude"),
            &["Thinking"],
            true,
            Some("claude"),
            Some("WaitingApproval"),
        )];
        let report = evaluate(&fixtures);
        // This should fail waiting recall since the poller will classify as Running
        let waiting = report
            .activity_metrics
            .iter()
            .find(|(n, _)| n == "WaitingApproval");
        let (_, m) = waiting.expect("WaitingApproval class should exist");
        assert_eq!(m.true_positives, 0);
        assert_eq!(m.false_negatives, 1);
        assert!((m.recall - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gate_thresholds_correct() {
        assert!((GATE_WEIGHTED_F1 - 0.85).abs() < f64::EPSILON);
        assert!((GATE_WAITING_RECALL - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn class_metrics_computation() {
        let mut m = ClassMetrics {
            true_positives: 8,
            false_positives: 2,
            false_negatives: 1,
            support: 9,
            ..Default::default()
        };
        m.compute();
        // precision = 8/10 = 0.8
        assert!((m.precision - 0.8).abs() < 0.01);
        // recall = 8/9 ≈ 0.889
        assert!((m.recall - 8.0 / 9.0).abs() < 0.01);
        // f1 = 2 * 0.8 * 0.889 / (0.8 + 0.889) ≈ 0.842
        let expected_f1 = 2.0 * 0.8 * (8.0 / 9.0) / (0.8 + 8.0 / 9.0);
        assert!((m.f1 - expected_f1).abs() < 0.01);
    }

    #[test]
    fn class_metrics_zero_support() {
        let mut m = ClassMetrics::default();
        m.compute();
        assert!((m.precision - 0.0).abs() < f64::EPSILON);
        assert!((m.recall - 0.0).abs() < f64::EPSILON);
        assert!((m.f1 - 0.0).abs() < f64::EPSILON);
    }

    // ── Fixture loading ────────────────────────────────────

    #[test]
    fn load_fixtures_nonexistent_file() {
        let result = load_fixtures("/nonexistent/path.json");
        assert!(result.is_err());
    }

    // ── Integration: evaluate against real fixture if available ──

    #[test]
    fn integration_fixture_gate() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/poller-baseline/dataset.json"
        );
        // FR-033: fixture must exist and be re-measured every time — no silent skip.
        let fixtures = load_fixtures(fixture_path).expect("fixture must exist and parse");
        assert!(
            fixtures.len() >= 300,
            "fixture must have >= 300 windows, got {}",
            fixtures.len()
        );

        let report = evaluate(&fixtures);
        print_report(&report);

        assert!(
            report.gate_pass,
            "Quality gate FAILED: weighted_f1={:.3} (>= {:.2}), waiting_recall={:.3} (>= {:.2})",
            report.weighted_f1, GATE_WEIGHTED_F1, report.waiting_recall, GATE_WAITING_RECALL,
        );
    }

    #[test]
    fn evaluate_all_wrong_fails_gate() {
        let fixtures = vec![
            // expected WaitingApproval but capture shows Running pattern
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Thinking"],
                true,
                Some("claude"),
                Some("WaitingApproval"),
            ),
            // expected Idle but capture shows Error pattern
            make_window(
                "claude code",
                "claude",
                Some("claude"),
                &["Error: crash"],
                true,
                Some("claude"),
                Some("Idle"),
            ),
        ];
        let report = evaluate(&fixtures);
        assert!(!report.gate_pass);
        assert!(report.weighted_f1 < GATE_WEIGHTED_F1);
    }
}
