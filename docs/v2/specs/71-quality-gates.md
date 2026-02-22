# AGTMUX v2 Quality Gates

Date: 2026-02-21  
Status: Active  
Depends on: `../20-unified-design.md`, `../30-detailed-design.md`, `../40-execution-plan.md`, `../60-ui-feedback-loop.md`

## 1. Purpose

「gate pass」を曖昧語で運用しないため、定量閾値を固定する。

## 2. Stage Policy

gate は段階導入する。

1. `Dev Gate` (Phase B-D)
2. `Beta Gate` (Phase E-F)
3. `Release Gate` (Phase G+)

PR は「所属phaseの gate」を満たせば通過可能とする。

## 3. Terminal Performance Gates

## 3.1 Dev Gate (Phase B-D)

Local target:

1. `input_latency_p95_ms < 35`
2. `active_fps_median >= 45`
3. `selected_stream_gap_p95_ms < 70`
4. `pane_switch_p95_ms < 180`

SSH target:

1. `input_latency_p95_ms < 120`
2. `selected_stream_gap_p95_ms < 180`

## 3.2 Beta Gate (Phase E-F)

Local target:

1. `input_latency_p95_ms < 25`
2. `active_fps_median >= 50`
3. `selected_stream_gap_p95_ms < 50`
4. `pane_switch_p95_ms < 140`

SSH target:

1. `input_latency_p95_ms < 100`
2. `selected_stream_gap_p95_ms < 140`

## 3.3 Release Gate (Phase G+)

Local target:

1. `input_latency_p95_ms < 20`
2. `active_fps_median >= 55`
3. `selected_stream_gap_p95_ms < 40`
4. `pane_switch_p95_ms < 120`

SSH target:

1. `input_latency_p95_ms < 80`
2. `selected_stream_gap_p95_ms < 120`

共通:

1. `selected_hotpath_capture_count == 0`

## 4. Layout Mutation Gates

1. `layout_mutation_success_rate >= 0.995`
2. `layout_mutation_timeout_rate <= 0.005`
3. `topology_divergence_count == 0`（replay suite）

## 5. State Precision Gates

評価対象: managed pane のみ  
評価セット: codex/claude/gemini fixture + 手動ラベル付けログ

## 5.1 Dev Gate

1. `activity_weighted_f1 >= 0.88`
2. `running_precision >= 0.92`
3. `waiting_input_recall >= 0.75`
4. `waiting_approval_recall >= 0.70`

## 5.2 Beta Gate

1. `activity_weighted_f1 >= 0.92`
2. `running_precision >= 0.95`
3. `waiting_input_recall >= 0.85`
4. `waiting_approval_recall >= 0.82`

## 5.3 Release Gate

1. `activity_weighted_f1 >= 0.95`
2. `running_precision >= 0.97`
3. `waiting_input_recall >= 0.90`
4. `waiting_approval_recall >= 0.90`

## 6. Attention Precision Gates

評価対象:

1. `task_complete`
2. `waiting_input`
3. `waiting_approval`
4. `error`

## 6.1 Dev Gate

1. `attention_precision >= 0.78`
2. `attention_recall >= 0.70`
3. `false_positive_rate <= 0.20`

## 6.2 Beta Gate

1. `attention_precision >= 0.85`
2. `attention_recall >= 0.80`
3. `false_positive_rate <= 0.14`

## 6.3 Release Gate

1. `attention_precision >= 0.90`
2. `attention_recall >= 0.85`
3. `false_positive_rate <= 0.10`

## 7. UI Feedback Loop Gates

UI変更PRで必須:

1. `scripts/ui-feedback/run-ui-feedback-report.sh 1` 実行
2. `tests_failures == 0`
3. report artifact をPRに添付

推奨:

1. 主要UI変更は `iterations=3`
2. `tests_skipped > 0` は理由記載必須

注意:

1. `ui_snapshot_errors` は fail 条件にしない（診断指標）

## 8. Release Blockers

次のいずれかで release blocker:

1. L0 invariant 違反
2. 上記 gate 未達
3. ADR と実装の不整合

## 9. Measurement Window

1. perf計測は同一環境で3run median
2. precision計測は固定fixture + 最新運用ログを併用
3. 大型変更後は baseline を更新して履歴保存
