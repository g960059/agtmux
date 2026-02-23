use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::label::RecordedEvent;

// ---------------------------------------------------------------------------
// Per-class metrics
// ---------------------------------------------------------------------------

/// Accuracy metrics for a single activity state class.
#[derive(Debug, Default, Clone)]
pub struct ClassMetrics {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
}

impl ClassMetrics {
    /// Precision = TP / (TP + FP). Returns 0.0 when TP + FP == 0.
    pub fn precision(&self) -> f64 {
        let denom = self.true_positives + self.false_positives;
        if denom == 0 {
            0.0
        } else {
            self.true_positives as f64 / denom as f64
        }
    }

    /// Recall = TP / (TP + FN). Returns 0.0 when TP + FN == 0.
    pub fn recall(&self) -> f64 {
        let denom = self.true_positives + self.false_negatives;
        if denom == 0 {
            0.0
        } else {
            self.true_positives as f64 / denom as f64
        }
    }

    /// F1 = 2 * P * R / (P + R). Returns 0.0 when P + R == 0.
    pub fn f1(&self) -> f64 {
        let p = self.precision();
        let r = self.recall();
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }

    /// Support = TP + FN (number of actual instances of this class).
    pub fn support(&self) -> usize {
        self.true_positives + self.false_negatives
    }
}

// ---------------------------------------------------------------------------
// Accuracy report
// ---------------------------------------------------------------------------

/// Overall accuracy report computed from labeled events.
pub struct AccuracyReport {
    /// Per-class metrics keyed by state name (e.g. "running", "idle").
    pub per_class: HashMap<String, ClassMetrics>,
    /// Total number of labeled samples used.
    pub total_samples: usize,
    /// Weighted F1 score (weighted by per-class support).
    pub weighted_f1: f64,
}

impl AccuracyReport {
    /// Compute an accuracy report from (predicted, actual) pairs.
    ///
    /// Both `predicted` and `actual` are activity state names such as
    /// "running", "waiting_input", etc.
    pub fn compute(events: &[(String, String)]) -> Self {
        let mut per_class: HashMap<String, ClassMetrics> = HashMap::new();

        // Collect all classes that appear in either predicted or actual.
        for (predicted, actual) in events {
            // Ensure both classes exist in the map.
            per_class.entry(predicted.clone()).or_default();
            per_class.entry(actual.clone()).or_default();
        }

        // Count TP, FP, FN per class.
        for (predicted, actual) in events {
            if predicted == actual {
                // True positive for this class.
                per_class.get_mut(predicted).unwrap().true_positives += 1;
            } else {
                // False positive for the predicted class.
                per_class.get_mut(predicted).unwrap().false_positives += 1;
                // False negative for the actual class.
                per_class.get_mut(actual).unwrap().false_negatives += 1;
            }
        }

        let total_samples = events.len();

        // Compute weighted F1: sum(f1_i * support_i) / sum(support_i).
        let total_support: usize = per_class.values().map(|m| m.support()).sum();
        let weighted_f1 = if total_support == 0 {
            0.0
        } else {
            let sum: f64 = per_class
                .values()
                .map(|m| m.f1() * m.support() as f64)
                .sum();
            sum / total_support as f64
        };

        AccuracyReport {
            per_class,
            total_samples,
            weighted_f1,
        }
    }
}

// ---------------------------------------------------------------------------
// Quality gate definitions
// ---------------------------------------------------------------------------

/// A single quality gate check.
struct GateCheck {
    name: &'static str,
    threshold: f64,
    actual: f64,
    higher_is_better: bool,
}

impl GateCheck {
    fn passed(&self) -> bool {
        if self.higher_is_better {
            self.actual >= self.threshold
        } else {
            self.actual <= self.threshold
        }
    }

    fn label(&self) -> &'static str {
        if self.passed() {
            "PASS"
        } else {
            "FAIL"
        }
    }
}

// ---------------------------------------------------------------------------
// Report printing
// ---------------------------------------------------------------------------

/// Print a formatted accuracy report to stdout.
pub fn print_report(report: &AccuracyReport) {
    // Header
    println!(
        "{:<24} {:>10} {:>10} {:>10} {:>10}",
        "Class", "Precision", "Recall", "F1", "Support"
    );

    // Sort classes for deterministic output.
    let mut classes: Vec<&String> = report.per_class.keys().collect();
    classes.sort();

    for class in &classes {
        let m = &report.per_class[*class];
        println!(
            "{:<24} {:>10.2} {:>10.2} {:>10.2} {:>10}",
            class,
            m.precision(),
            m.recall(),
            m.f1(),
            m.support(),
        );
    }

    println!("{}", "\u{2500}".repeat(66));
    println!("Weighted F1: {:.4}", report.weighted_f1);
    println!("Total samples: {}", report.total_samples);
    println!();

    // Quality gate checks
    let running_precision = report
        .per_class
        .get("running")
        .map(|m| m.precision())
        .unwrap_or(0.0);
    let waiting_input_recall = report
        .per_class
        .get("waiting_input")
        .map(|m| m.recall())
        .unwrap_or(0.0);
    let waiting_approval_recall = report
        .per_class
        .get("waiting_approval")
        .map(|m| m.recall())
        .unwrap_or(0.0);

    let dev_gates = vec![
        GateCheck {
            name: "activity_weighted_f1 >= 0.88",
            threshold: 0.88,
            actual: report.weighted_f1,
            higher_is_better: true,
        },
        GateCheck {
            name: "running_precision >= 0.92",
            threshold: 0.92,
            actual: running_precision,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_input_recall >= 0.75",
            threshold: 0.75,
            actual: waiting_input_recall,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_approval_recall >= 0.70",
            threshold: 0.70,
            actual: waiting_approval_recall,
            higher_is_better: true,
        },
    ];

    let beta_gates = vec![
        GateCheck {
            name: "activity_weighted_f1 >= 0.92",
            threshold: 0.92,
            actual: report.weighted_f1,
            higher_is_better: true,
        },
        GateCheck {
            name: "running_precision >= 0.95",
            threshold: 0.95,
            actual: running_precision,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_input_recall >= 0.85",
            threshold: 0.85,
            actual: waiting_input_recall,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_approval_recall >= 0.82",
            threshold: 0.82,
            actual: waiting_approval_recall,
            higher_is_better: true,
        },
    ];

    let release_gates = vec![
        GateCheck {
            name: "activity_weighted_f1 >= 0.95",
            threshold: 0.95,
            actual: report.weighted_f1,
            higher_is_better: true,
        },
        GateCheck {
            name: "running_precision >= 0.97",
            threshold: 0.97,
            actual: running_precision,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_input_recall >= 0.90",
            threshold: 0.90,
            actual: waiting_input_recall,
            higher_is_better: true,
        },
        GateCheck {
            name: "waiting_approval_recall >= 0.90",
            threshold: 0.90,
            actual: waiting_approval_recall,
            higher_is_better: true,
        },
    ];

    println!("Quality Gates:");
    println!("  Dev:");
    for gate in &dev_gates {
        println!(
            "    [{}] {} (actual: {:.4})",
            gate.label(),
            gate.name,
            gate.actual
        );
    }
    println!("  Beta:");
    for gate in &beta_gates {
        println!(
            "    [{}] {} (actual: {:.4})",
            gate.label(),
            gate.name,
            gate.actual
        );
    }
    println!("  Release:");
    for gate in &release_gates {
        println!(
            "    [{}] {} (actual: {:.4})",
            gate.label(),
            gate.name,
            gate.actual
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers for extracting predicted state from recorded data
// ---------------------------------------------------------------------------

/// Extract the predicted activity state from a recorded event's data field.
///
/// Looks for `data.state.activity.state` (the structure emitted by the
/// orchestrator's `StateNotification::StateChanged`).
pub fn extract_predicted_state(data: &serde_json::Value) -> Option<String> {
    data.get("state")
        .and_then(|s| s.get("activity"))
        .and_then(|a| a.get("state"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the accuracy analysis on a labeled JSONL file.
pub fn run_accuracy(input_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(input_path)?;
    let reader = BufReader::new(file);

    let mut pairs: Vec<(String, String)> = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }

        let event: RecordedEvent = serde_json::from_str(&line).map_err(|e| {
            format!("line {}: failed to parse JSON: {}", line_num + 1, e)
        })?;

        // Only process StateChanged events that have a label.
        if event.event_type != "StateChanged" {
            continue;
        }

        let label = match &event.label {
            Some(l) => l.clone(),
            None => continue, // unlabeled event, skip
        };

        let predicted = match extract_predicted_state(&event.data) {
            Some(p) => p,
            None => {
                eprintln!(
                    "warning: line {}: could not extract predicted state, skipping",
                    line_num + 1
                );
                continue;
            }
        };

        pairs.push((predicted, label));
    }

    if pairs.is_empty() {
        eprintln!("No labeled StateChanged events found in {:?}", input_path);
        return Ok(());
    }

    let report = AccuracyReport::compute(&pairs);
    print_report(&report);

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create (predicted, actual) pairs from short labels.
    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(p, a)| (p.to_string(), a.to_string()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // ClassMetrics unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn precision_basic() {
        let m = ClassMetrics {
            true_positives: 8,
            false_positives: 2,
            false_negatives: 0,
        };
        assert!((m.precision() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn recall_basic() {
        let m = ClassMetrics {
            true_positives: 6,
            false_positives: 0,
            false_negatives: 4,
        };
        assert!((m.recall() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn f1_basic() {
        // P = 8/10 = 0.8, R = 8/12 = 0.667, F1 = 2*0.8*0.667/(0.8+0.667) = 0.7273
        let m = ClassMetrics {
            true_positives: 8,
            false_positives: 2,
            false_negatives: 4,
        };
        let expected_p = 8.0 / 10.0;
        let expected_r = 8.0 / 12.0;
        let expected_f1 = 2.0 * expected_p * expected_r / (expected_p + expected_r);
        assert!((m.f1() - expected_f1).abs() < 1e-9);
    }

    #[test]
    fn precision_zero_denominator() {
        let m = ClassMetrics {
            true_positives: 0,
            false_positives: 0,
            false_negatives: 5,
        };
        assert!((m.precision() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn recall_zero_denominator() {
        let m = ClassMetrics {
            true_positives: 0,
            false_positives: 5,
            false_negatives: 0,
        };
        assert!((m.recall() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn f1_zero_when_both_zero() {
        let m = ClassMetrics {
            true_positives: 0,
            false_positives: 0,
            false_negatives: 0,
        };
        assert!((m.f1() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn support_counts_actual_instances() {
        let m = ClassMetrics {
            true_positives: 7,
            false_positives: 3,
            false_negatives: 2,
        };
        assert_eq!(m.support(), 9); // 7 + 2
    }

    // -----------------------------------------------------------------------
    // AccuracyReport tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_perfect_predictions() {
        let data = pairs(&[
            ("running", "running"),
            ("running", "running"),
            ("idle", "idle"),
            ("waiting_input", "waiting_input"),
            ("error", "error"),
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 5);
        assert!((report.weighted_f1 - 1.0).abs() < 1e-9);

        for (_class, m) in &report.per_class {
            assert!((m.precision() - 1.0).abs() < 1e-9);
            assert!((m.recall() - 1.0).abs() < 1e-9);
            assert!((m.f1() - 1.0).abs() < 1e-9);
            assert_eq!(m.false_positives, 0);
            assert_eq!(m.false_negatives, 0);
        }
    }

    #[test]
    fn test_all_wrong() {
        // Every prediction is wrong: predicted A when actual is B and vice versa.
        let data = pairs(&[
            ("running", "idle"),
            ("idle", "running"),
            ("running", "idle"),
            ("idle", "running"),
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 4);
        assert!((report.weighted_f1 - 0.0).abs() < 1e-9);

        for (_class, m) in &report.per_class {
            assert_eq!(m.true_positives, 0);
            assert!((m.f1() - 0.0).abs() < 1e-9);
        }
    }

    #[test]
    fn test_mixed_predictions() {
        // 3 running correct, 1 running wrong (predicted running, actual idle)
        // 2 idle correct, 1 idle wrong (predicted idle, actual running)
        let data = pairs(&[
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            ("running", "idle"),     // FP for running, FN for idle
            ("idle", "idle"),
            ("idle", "idle"),
            ("idle", "running"),     // FP for idle, FN for running
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 7);

        let running = &report.per_class["running"];
        assert_eq!(running.true_positives, 3);
        assert_eq!(running.false_positives, 1);
        assert_eq!(running.false_negatives, 1);
        // P = 3/4 = 0.75, R = 3/4 = 0.75, F1 = 0.75
        assert!((running.precision() - 0.75).abs() < 1e-9);
        assert!((running.recall() - 0.75).abs() < 1e-9);
        assert!((running.f1() - 0.75).abs() < 1e-9);
        assert_eq!(running.support(), 4); // TP + FN = 3 + 1

        let idle = &report.per_class["idle"];
        assert_eq!(idle.true_positives, 2);
        assert_eq!(idle.false_positives, 1);
        assert_eq!(idle.false_negatives, 1);
        // P = 2/3, R = 2/3, F1 = 2/3
        assert!((idle.precision() - 2.0 / 3.0).abs() < 1e-9);
        assert!((idle.recall() - 2.0 / 3.0).abs() < 1e-9);
        assert!((idle.f1() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(idle.support(), 3); // TP + FN = 2 + 1

        // Weighted F1: (0.75 * 4 + 0.667 * 3) / 7
        let expected_wf1 = (0.75 * 4.0 + (2.0 / 3.0) * 3.0) / 7.0;
        assert!((report.weighted_f1 - expected_wf1).abs() < 1e-9);
    }

    #[test]
    fn test_single_class() {
        let data = pairs(&[
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 3);
        assert_eq!(report.per_class.len(), 1);
        assert!((report.weighted_f1 - 1.0).abs() < 1e-9);

        let running = &report.per_class["running"];
        assert_eq!(running.true_positives, 3);
        assert_eq!(running.false_positives, 0);
        assert_eq!(running.false_negatives, 0);
    }

    #[test]
    fn test_precision_recall_calculation() {
        // Scenario: system predicts "running" 10 times, but only 7 are correct.
        // The remaining 3 actual "running" instances were predicted as something else.
        let data = pairs(&[
            // 7 correct running
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            ("running", "running"),
            // 3 FP for running (predicted running, actual idle)
            ("running", "idle"),
            ("running", "idle"),
            ("running", "idle"),
            // 3 FN for running (predicted idle, actual running)
            ("idle", "running"),
            ("idle", "running"),
            ("idle", "running"),
            // 2 correct idle
            ("idle", "idle"),
            ("idle", "idle"),
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 15);

        let running = &report.per_class["running"];
        assert_eq!(running.true_positives, 7);
        assert_eq!(running.false_positives, 3);
        assert_eq!(running.false_negatives, 3);
        // P = 7/10, R = 7/10
        assert!((running.precision() - 0.7).abs() < 1e-9);
        assert!((running.recall() - 0.7).abs() < 1e-9);
        assert!((running.f1() - 0.7).abs() < 1e-9);
        assert_eq!(running.support(), 10);

        let idle = &report.per_class["idle"];
        assert_eq!(idle.true_positives, 2);
        assert_eq!(idle.false_positives, 3);
        assert_eq!(idle.false_negatives, 3);
        // P = 2/5, R = 2/5
        assert!((idle.precision() - 0.4).abs() < 1e-9);
        assert!((idle.recall() - 0.4).abs() < 1e-9);
        assert!((idle.f1() - 0.4).abs() < 1e-9);
        assert_eq!(idle.support(), 5);
    }

    #[test]
    fn test_weighted_f1_calculation() {
        // Three classes with different support and different F1 scores.
        // Class A: 4 correct, 1 wrong => P=4/5, R=4/5, F1=0.8, support=5
        //   (predicted A, actual B) => FP(A), FN(B)
        //   (predicted B, actual A) => FP(B), FN(A)
        // We will construct specific data to get known values.
        //
        // A: 4 TP, 1 FP (predicted A actual B), 1 FN (predicted C actual A)
        //   => P = 4/5 = 0.8, R = 4/5 = 0.8, F1 = 0.8, support = 5
        // B: 3 TP, 1 FP (predicted B actual C), 1 FN (predicted A actual B)
        //   => P = 3/4 = 0.75, R = 3/4 = 0.75, F1 = 0.75, support = 4
        // C: 2 TP, 1 FP (predicted C actual A), 1 FN (predicted B actual C)
        //   => P = 2/3, R = 2/3, F1 = 2/3, support = 3
        let data = pairs(&[
            ("A", "A"),
            ("A", "A"),
            ("A", "A"),
            ("A", "A"),
            ("A", "B"), // FP(A), FN(B)
            ("B", "B"),
            ("B", "B"),
            ("B", "B"),
            ("B", "C"), // FP(B), FN(C)
            ("C", "C"),
            ("C", "C"),
            ("C", "A"), // FP(C), FN(A)
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 12);

        let a = &report.per_class["A"];
        assert_eq!(a.true_positives, 4);
        assert_eq!(a.false_positives, 1);
        assert_eq!(a.false_negatives, 1);
        assert!((a.f1() - 0.8).abs() < 1e-9);
        assert_eq!(a.support(), 5);

        let b = &report.per_class["B"];
        assert_eq!(b.true_positives, 3);
        assert_eq!(b.false_positives, 1);
        assert_eq!(b.false_negatives, 1);
        assert!((b.f1() - 0.75).abs() < 1e-9);
        assert_eq!(b.support(), 4);

        let c = &report.per_class["C"];
        assert_eq!(c.true_positives, 2);
        assert_eq!(c.false_positives, 1);
        assert_eq!(c.false_negatives, 1);
        let c_f1 = 2.0 / 3.0;
        assert!((c.f1() - c_f1).abs() < 1e-9);
        assert_eq!(c.support(), 3);

        // Weighted F1 = (0.8*5 + 0.75*4 + (2/3)*3) / (5+4+3)
        //             = (4.0 + 3.0 + 2.0) / 12
        //             = 9.0 / 12 = 0.75
        let expected = (0.8 * 5.0 + 0.75 * 4.0 + c_f1 * 3.0) / 12.0;
        assert!((report.weighted_f1 - expected).abs() < 1e-9);
    }

    #[test]
    fn test_empty_input() {
        let data: Vec<(String, String)> = Vec::new();
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 0);
        assert!((report.weighted_f1 - 0.0).abs() < 1e-9);
        assert!(report.per_class.is_empty());
    }

    #[test]
    fn test_multiclass_with_zero_support_class() {
        // Class "error" only appears as a prediction (FP), never as actual.
        // It should have FP > 0 but support = 0 (TP + FN = 0).
        let data = pairs(&[
            ("running", "running"),
            ("error", "running"), // FP for error, FN for running
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 2);

        let error = &report.per_class["error"];
        assert_eq!(error.true_positives, 0);
        assert_eq!(error.false_positives, 1);
        assert_eq!(error.false_negatives, 0);
        assert_eq!(error.support(), 0);
        assert!((error.f1() - 0.0).abs() < 1e-9);

        let running = &report.per_class["running"];
        assert_eq!(running.true_positives, 1);
        assert_eq!(running.false_positives, 0);
        assert_eq!(running.false_negatives, 1);
        assert_eq!(running.support(), 2);
    }

    #[test]
    fn test_extract_predicted_state() {
        let data = serde_json::json!({
            "pane_id": "%1",
            "state": {
                "activity": {
                    "state": "running",
                    "confidence": 0.95,
                    "source": "hook",
                    "reason_code": "tool_running"
                }
            }
        });
        assert_eq!(
            extract_predicted_state(&data),
            Some("running".to_string())
        );
    }

    #[test]
    fn test_extract_predicted_state_missing_field() {
        let data = serde_json::json!({"pane_id": "%1"});
        assert_eq!(extract_predicted_state(&data), None);
    }

    #[test]
    fn test_all_six_states_perfect() {
        let data = pairs(&[
            ("unknown", "unknown"),
            ("idle", "idle"),
            ("running", "running"),
            ("waiting_input", "waiting_input"),
            ("waiting_approval", "waiting_approval"),
            ("error", "error"),
        ]);
        let report = AccuracyReport::compute(&data);

        assert_eq!(report.total_samples, 6);
        assert!((report.weighted_f1 - 1.0).abs() < 1e-9);
        assert_eq!(report.per_class.len(), 6);
    }
}
