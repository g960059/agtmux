# Poller Fallback Quality Baseline Specification

Task ref: T-033 | Spec: FR-032, FR-033 | User stories: US-002, US-005

## Purpose

This document defines the acceptance criteria for the poller heuristic
fallback source, ensuring minimum quality before release.

---

## Quality Gates

| Metric | Threshold | Rationale |
|--------|-----------|-----------|
| **Weighted F1** | >= 0.85 | Overall classification accuracy weighted by class support |
| **WaitingApproval recall** | >= 0.85 | Critical UX signal — user must see approval prompts |

Both gates must pass simultaneously. Failure of either gate blocks release.

## Evaluation Dataset

| Property | Requirement |
|----------|-------------|
| Location | `fixtures/poller-baseline/dataset.json` |
| Minimum size | >= 300 labeled windows |
| Provider mix | ~40% Claude, ~40% Codex, ~20% no-agent |
| Activity distribution | Running, Idle, WaitingApproval, Error, Unknown |
| Stability | Dataset is frozen at spec time; changes require new T-033 cycle |

### Labeled Window Schema

```json
{
  "pane_id": "%N",
  "pane_title": "string",
  "current_cmd": "string",
  "process_hint": "string | null",
  "capture_lines": ["string", ...],
  "expected_detected": true | false,
  "expected_provider": "claude" | "codex" | null,
  "expected_activity": "Running" | "Idle" | "WaitingApproval" | "Error" | "Unknown" | null
}
```

## Metrics Definition

### Weighted F1

Weighted F1 is the support-weighted average of per-class F1 scores:

```
weighted_f1 = Σ(F1_class × support_class) / Σ(support_class)
```

where:
- Classes: Running, Idle, WaitingApproval, Error, Unknown
- F1 = 2 × precision × recall / (precision + recall)
- Support = number of ground-truth instances per class
- Only windows with `expected_detected = true` contribute to activity metrics

### WaitingApproval Recall

```
waiting_recall = TP_waiting / (TP_waiting + FN_waiting)
```

This specifically measures the poller's ability to detect approval/confirmation
states, which is the highest-impact UX signal (user needs to see and act on prompts).

### Detection Accuracy

Supplementary metric (not gated):
```
detection_accuracy = correct_detected / total_windows
```

## Gate Command

```bash
just poller-gate
```

This runs `cargo test -p agtmux-source-poller integration_fixture_gate -- --nocapture`
which:
1. Loads `fixtures/poller-baseline/dataset.json`.
2. Runs `poll_pane()` on each window.
3. Computes per-class metrics and weighted F1.
4. Prints a diagnostic report.
5. Asserts `weighted_f1 >= 0.85` AND `waiting_recall >= 0.85`.

## Implementation Reference

| Component | File | Role |
|-----------|------|------|
| Detection | `crates/agtmux-source-poller/src/detect.rs` | Provider pattern matching |
| Evidence | `crates/agtmux-source-poller/src/evidence.rs` | Activity signal matching |
| Source server | `crates/agtmux-source-poller/src/source.rs` | `poll_pane()` integration |
| Evaluator | `crates/agtmux-source-poller/src/accuracy.rs` | Metrics + gate logic |

## Signal Weights (hardcoded in core)

| Signal | Weight | Source | Note |
|--------|--------|--------|------|
| `process_hint` | 1.00 | `WEIGHT_PROCESS_HINT` | |
| `cmd_match` | 0.86 | `WEIGHT_CMD_MATCH` | |
| `poller_match` | 0.78 | `WEIGHT_POLLER_MATCH` | capture-based detection (4th signal) |
| `title_match` | 0.66 | `WEIGHT_TITLE_MATCH` | title-only では検出しない（無条件抑制） |

## Activity Precedence (conflict resolution)

```
Error > WaitingApproval > Running > Idle > WaitingInput > Unknown
```

Higher-priority states win when multiple signals match on the same pane.

## Re-measurement Policy

- The fixture dataset is re-used for every gate check (stable benchmark).
- If detection logic changes, re-run `just poller-gate` before merge.
- If the fixture needs updating (e.g., new provider), create a new T-033 cycle.
- Weight/threshold changes require ADR + re-measurement.
