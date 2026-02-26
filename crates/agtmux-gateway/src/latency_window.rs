//! Rolling window p95 latency evaluator with SLO breach detection
//! and degraded alert support.
//!
//! Task ref: T-043

// ─── Types ──────────────────────────────────────────────────────────

/// Result of evaluating the current latency window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatencyEvaluation {
    /// Not enough samples for meaningful evaluation.
    InsufficientData {
        sample_count: usize,
        min_required: usize,
    },
    /// P95 is within SLO.
    Healthy { p95_ms: u64 },
    /// P95 exceeds SLO but below breach threshold.
    Breached {
        p95_ms: u64,
        consecutive: u32,
        threshold: u32,
    },
    /// Consecutive breaches reached threshold — degraded alert.
    Degraded { p95_ms: u64, consecutive: u32 },
}

// ─── Internal ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatencySample {
    latency_ms: u64,
    timestamp_ms: u64,
}

// ─── LatencyWindow ──────────────────────────────────────────────────

/// Rolling window latency tracker with SLO breach detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyWindow {
    /// Latency samples (milliseconds) in chronological order.
    samples: Vec<LatencySample>,
    /// Window duration in milliseconds (default 600_000 = 10min).
    window_ms: u64,
    /// Minimum number of events required for evaluation (default 200).
    min_events: usize,
    /// P95 SLO threshold in milliseconds.
    slo_threshold_ms: u64,
    /// Consecutive breach count for degraded alert.
    consecutive_breaches: u32,
    /// Breach threshold for degraded state (default 3).
    breach_threshold: u32,
}

impl LatencyWindow {
    /// Create with defaults: 10min window, 200 min events, 3 breach threshold.
    pub fn new(slo_threshold_ms: u64) -> Self {
        Self {
            samples: Vec::new(),
            window_ms: 600_000,
            min_events: 200,
            slo_threshold_ms,
            consecutive_breaches: 0,
            breach_threshold: 3,
        }
    }

    /// Create with fully custom configuration.
    pub fn with_config(
        slo_threshold_ms: u64,
        window_ms: u64,
        min_events: usize,
        breach_threshold: u32,
    ) -> Self {
        Self {
            samples: Vec::new(),
            window_ms,
            min_events,
            slo_threshold_ms,
            consecutive_breaches: 0,
            breach_threshold,
        }
    }

    /// Record a latency observation.
    pub fn record(&mut self, latency_ms: u64, now_ms: u64) {
        self.samples.push(LatencySample {
            latency_ms,
            timestamp_ms: now_ms,
        });
    }

    /// Evaluate the current window. Prunes old samples, computes p95.
    /// Returns evaluation result.
    pub fn evaluate(&mut self, now_ms: u64) -> LatencyEvaluation {
        // 1. Prune samples outside window
        let cutoff = now_ms.saturating_sub(self.window_ms);
        self.samples.retain(|s| s.timestamp_ms >= cutoff);

        // 2. Check minimum event count
        let count = self.samples.len();
        if count < self.min_events {
            return LatencyEvaluation::InsufficientData {
                sample_count: count,
                min_required: self.min_events,
            };
        }

        // 3. Compute p95
        let p95 = compute_p95(&self.samples);

        // 4. Evaluate against SLO
        if p95 <= self.slo_threshold_ms {
            self.consecutive_breaches = 0;
            LatencyEvaluation::Healthy { p95_ms: p95 }
        } else {
            self.consecutive_breaches = self.consecutive_breaches.saturating_add(1);
            if self.consecutive_breaches >= self.breach_threshold {
                LatencyEvaluation::Degraded {
                    p95_ms: p95,
                    consecutive: self.consecutive_breaches,
                }
            } else {
                LatencyEvaluation::Breached {
                    p95_ms: p95,
                    consecutive: self.consecutive_breaches,
                    threshold: self.breach_threshold,
                }
            }
        }
    }

    /// Current consecutive breach count.
    pub fn consecutive_breaches(&self) -> u32 {
        self.consecutive_breaches
    }

    /// Number of samples in the current window.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

// ─── P95 Calculation ────────────────────────────────────────────────

/// Compute p95 from a non-empty slice of samples.
///
/// P95 index = ceil(0.95 * count) - 1 (0-based).
fn compute_p95(samples: &[LatencySample]) -> u64 {
    let mut latencies: Vec<u64> = samples.iter().map(|s| s.latency_ms).collect();
    latencies.sort_unstable();

    let count = latencies.len();
    // ceil(0.95 * count) - 1, computed with integer arithmetic to avoid floats.
    // ceil(0.95 * count) = ceil(95 * count / 100) = (95 * count + 99) / 100
    let p95_index = (95 * count).div_ceil(100) - 1;

    latencies[p95_index]
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. empty_window_insufficient_data ───────────────────────────

    #[test]
    fn empty_window_insufficient_data() {
        let mut lw = LatencyWindow::new(100);
        let result = lw.evaluate(1_000_000);
        assert_eq!(
            result,
            LatencyEvaluation::InsufficientData {
                sample_count: 0,
                min_required: 200,
            }
        );
    }

    // ── 2. below_min_events_insufficient ────────────────────────────

    #[test]
    fn below_min_events_insufficient() {
        let mut lw = LatencyWindow::new(100);
        let base = 1_000_000;
        for i in 0..50 {
            lw.record(10, base + i);
        }
        let result = lw.evaluate(base + 50);
        assert_eq!(
            result,
            LatencyEvaluation::InsufficientData {
                sample_count: 50,
                min_required: 200,
            }
        );
    }

    // ── 3. healthy_p95_within_slo ───────────────────────────────────

    #[test]
    fn healthy_p95_within_slo() {
        let mut lw = LatencyWindow::new(100);
        let base = 1_000_000;
        // All samples at 50ms — well within 100ms SLO
        for i in 0..200 {
            lw.record(50, base + i);
        }
        let result = lw.evaluate(base + 200);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 50 });
    }

    // ── 4. p95_calculation_accuracy ─────────────────────────────────

    #[test]
    fn p95_calculation_accuracy() {
        // 100 samples with latencies 1..=100
        // p95 index = ceil(0.95 * 100) - 1 = 95 - 1 = 94 (0-based)
        // Sorted: [1,2,...,100], index 94 = 95
        let mut lw = LatencyWindow::with_config(200, 600_000, 100, 3);
        let base = 1_000_000;
        for i in 1..=100 {
            lw.record(i, base + i);
        }
        let result = lw.evaluate(base + 101);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 95 });
    }

    // ── 5. breach_on_high_p95 ───────────────────────────────────────

    #[test]
    fn breach_on_high_p95() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 200, 3);
        let base = 1_000_000;
        // 180 fast samples + 20 very slow samples → p95 index 189 lands on 500ms
        for i in 0..180 {
            lw.record(10, base + i);
        }
        for i in 180..200 {
            lw.record(500, base + i);
        }
        let result = lw.evaluate(base + 200);
        assert_eq!(
            result,
            LatencyEvaluation::Breached {
                p95_ms: 500,
                consecutive: 1,
                threshold: 3,
            }
        );
    }

    // ── 6. consecutive_breach_increments ─────────────────────────────

    #[test]
    fn consecutive_breach_increments() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 10, 5);
        let base = 1_000_000;

        // First evaluation — breach
        for i in 0..10 {
            lw.record(200, base + i);
        }
        let r1 = lw.evaluate(base + 10);
        assert_eq!(
            r1,
            LatencyEvaluation::Breached {
                p95_ms: 200,
                consecutive: 1,
                threshold: 5,
            }
        );

        // Second evaluation — breach again
        for i in 10..20 {
            lw.record(200, base + i);
        }
        let r2 = lw.evaluate(base + 20);
        assert_eq!(
            r2,
            LatencyEvaluation::Breached {
                p95_ms: 200,
                consecutive: 2,
                threshold: 5,
            }
        );
    }

    // ── 7. degraded_after_three_breaches ─────────────────────────────

    #[test]
    fn degraded_after_three_breaches() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 10, 3);
        let base = 1_000_000;

        for eval in 0..3 {
            let offset = eval * 10;
            for i in 0..10 {
                lw.record(200, base + offset as u64 + i);
            }
            let result = lw.evaluate(base + offset as u64 + 10);
            if eval < 2 {
                assert!(matches!(result, LatencyEvaluation::Breached { .. }));
            } else {
                assert_eq!(
                    result,
                    LatencyEvaluation::Degraded {
                        p95_ms: 200,
                        consecutive: 3,
                    }
                );
            }
        }
    }

    // ── 8. healthy_resets_breach_count ───────────────────────────────

    #[test]
    fn healthy_resets_breach_count() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 10, 3);
        let base = 1_000_000;

        // First eval — breach
        for i in 0..10 {
            lw.record(200, base + i);
        }
        lw.evaluate(base + 10);
        assert_eq!(lw.consecutive_breaches(), 1);

        // Second eval — healthy (add fast samples, old slow ones still in window)
        // Use a fresh window for clarity
        let mut lw2 = LatencyWindow::with_config(100, 600_000, 10, 3);
        let base2 = 2_000_000;
        for i in 0..10 {
            lw2.record(200, base2 + i);
        }
        lw2.evaluate(base2 + 10);
        assert_eq!(lw2.consecutive_breaches(), 1);

        // Now record enough fast samples and evaluate
        for i in 10..20 {
            lw2.record(50, base2 + i);
        }
        // Evaluate — the mix still has slow + fast; let's force a clear window
        // by using very small window so old slow samples are pruned
        let mut lw3 = LatencyWindow::with_config(100, 100, 10, 3);
        let base3 = 3_000_000;
        for i in 0..10 {
            lw3.record(200, base3 + i);
        }
        lw3.evaluate(base3 + 10);
        assert_eq!(lw3.consecutive_breaches(), 1);

        // Record fast samples much later so old ones get pruned
        for i in 0..10 {
            lw3.record(50, base3 + 200 + i);
        }
        let result = lw3.evaluate(base3 + 210);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 50 });
        assert_eq!(lw3.consecutive_breaches(), 0);
    }

    // ── 9. old_samples_pruned ───────────────────────────────────────

    #[test]
    fn old_samples_pruned() {
        let mut lw = LatencyWindow::with_config(100, 1000, 1, 3);

        // Record sample at t=100
        lw.record(50, 100);
        assert_eq!(lw.sample_count(), 1);

        // Evaluate at t=1200 — sample at t=100 is outside window [200, 1200]
        lw.evaluate(1200);
        assert_eq!(lw.sample_count(), 0);
    }

    // ── 10. window_pruning_leaves_recent ─────────────────────────────

    #[test]
    fn window_pruning_leaves_recent() {
        let mut lw = LatencyWindow::with_config(100, 1000, 1, 3);

        // Record old sample at t=100
        lw.record(50, 100);
        // Record recent sample at t=1100
        lw.record(60, 1100);
        // Record recent sample at t=1150
        lw.record(70, 1150);

        assert_eq!(lw.sample_count(), 3);

        // Evaluate at t=1200 — window is [200, 1200], only t=1100 and t=1150 survive
        let result = lw.evaluate(1200);
        assert_eq!(lw.sample_count(), 2);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 70 });
    }

    // ── 11. record_and_evaluate_cycle ────────────────────────────────

    #[test]
    fn record_and_evaluate_cycle() {
        let mut lw = LatencyWindow::with_config(100, 10_000, 5, 2);
        let base = 1_000_000;

        // Record 5 fast samples
        for i in 0..5 {
            lw.record(30, base + i);
        }

        // Eval 1: healthy
        let r1 = lw.evaluate(base + 5);
        assert_eq!(r1, LatencyEvaluation::Healthy { p95_ms: 30 });
        assert_eq!(lw.consecutive_breaches(), 0);

        // Record 5 slow samples
        for i in 5..10 {
            lw.record(200, base + i);
        }

        // Eval 2: breached (mix of fast and slow — p95 will hit slow tail)
        let r2 = lw.evaluate(base + 10);
        assert!(matches!(r2, LatencyEvaluation::Breached { .. }));
        assert_eq!(lw.consecutive_breaches(), 1);

        // Record more slow samples
        for i in 10..15 {
            lw.record(200, base + i);
        }

        // Eval 3: degraded (consecutive breaches >= 2)
        let r3 = lw.evaluate(base + 15);
        assert!(matches!(r3, LatencyEvaluation::Degraded { .. }));
        assert_eq!(lw.consecutive_breaches(), 2);
    }

    // ── 12. custom_config ────────────────────────────────────────────

    #[test]
    fn custom_config() {
        let lw = LatencyWindow::with_config(500, 30_000, 50, 5);
        assert_eq!(lw.sample_count(), 0);
        assert_eq!(lw.consecutive_breaches(), 0);

        // Verify fields through behavior
        // min_events = 50: with 49 samples should be InsufficientData
        let mut lw = LatencyWindow::with_config(500, 30_000, 50, 5);
        let base = 1_000_000;
        for i in 0..49 {
            lw.record(10, base + i);
        }
        let result = lw.evaluate(base + 49);
        assert_eq!(
            result,
            LatencyEvaluation::InsufficientData {
                sample_count: 49,
                min_required: 50,
            }
        );

        // With 50 samples it should evaluate
        lw.record(10, base + 49);
        let result = lw.evaluate(base + 50);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 10 });
    }

    // ── 13. single_sample_sufficient_if_min_is_one ───────────────────

    #[test]
    fn single_sample_sufficient_if_min_is_one() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 1, 3);
        lw.record(42, 1_000_000);
        let result = lw.evaluate(1_000_001);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 42 });
    }

    // ── 14. all_same_latency ─────────────────────────────────────────

    #[test]
    fn all_same_latency() {
        let mut lw = LatencyWindow::new(100);
        let base = 1_000_000;
        for i in 0..200 {
            lw.record(77, base + i);
        }
        let result = lw.evaluate(base + 200);
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 77 });
    }

    // ── 15. boundary_slo_exactly_at_threshold ────────────────────────

    #[test]
    fn boundary_slo_exactly_at_threshold() {
        let mut lw = LatencyWindow::with_config(100, 600_000, 10, 3);
        let base = 1_000_000;
        // All samples at exactly the SLO threshold
        for i in 0..10 {
            lw.record(100, base + i);
        }
        let result = lw.evaluate(base + 10);
        // p95 == threshold → Healthy (not breached)
        assert_eq!(result, LatencyEvaluation::Healthy { p95_ms: 100 });
        assert_eq!(lw.consecutive_breaches(), 0);
    }
}
