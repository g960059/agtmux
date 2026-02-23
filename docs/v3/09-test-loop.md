# State Accuracy: テストループ設計

## 0. 要約

v3 では「推定ロジック」だけでなく「推定の正しさを測る仕組み」を最初から組み込む。
Go POC のアルゴリズム構造は参考にするが、weight/confidence 数値は独自にテスト駆動で決定する。

---

## 1. テスト戦略概要

### 3+1 層テスト

```
Layer 1: Unit Fixtures (自動, CI, fast)
  └── JSON fixtures → Engine → assert expected output
  └── 実行: cargo test

Layer 2: Replay Scenarios (自動, CI, medium)
  └── 時系列 state transition sequences → Engine → assert transitions
  └── 実行: cargo test --test replay

Layer 3: Property-Based Tests (自動, CI, fast)
  └── proptest で不変条件を検証
  └── 実行: cargo test (proptest integration)

Layer 4: Live Validation (手動 → 半自動, Phase 3+)
  └── 実 tmux session → daemon → recorded ground truth → accuracy report
  └── 実行: agtmux record + agtmux accuracy
```

---

## 2. Layer 1: Unit Fixture Tests

### Fixture Format

```json
{
  "name": "claude_approval_via_hook",
  "description": "Claude code requests tool approval via hook event",
  "evidence": [
    {
      "provider": "claude",
      "kind": {"HookEvent": "needs-approval"},
      "signal": "WaitingApproval",
      "weight": 0.95,
      "confidence": 0.92,
      "timestamp": "2026-02-22T10:00:00Z",
      "ttl_secs": 90,
      "source": "Hook",
      "reason_code": "needs-approval"
    }
  ],
  "now": "2026-02-22T10:00:01Z",
  "expected": {
    "activity_state": "WaitingApproval",
    "confidence_min": 0.9
  }
}
```

**Note**: Engine は Evidence のみ受け取る設計のため、fixture も Evidence 配列を直接入力とする。

### Fixture カテゴリ

```
fixtures/
├── claude/
│   ├── hook_approval.json           # hook → WaitingApproval
│   ├── hook_input.json              # hook → WaitingInput
│   ├── hook_running.json            # hook → Running
│   ├── hook_done.json               # hook → Idle
│   ├── hook_error.json              # hook → Error
│   ├── poller_running.json          # poller → Running (legitimate)
│   ├── poller_false_positive.json   # poller → Running but suppressed to Idle
│   ├── poller_idle.json             # poller → Idle
│   └── no_signal.json              # no evidence → Unknown
├── codex/
│   ├── api_approval.json            # API → WaitingApproval
│   ├── api_running.json             # API → Running
│   ├── api_error.json               # API → Error
│   ├── notify_running.json          # notify → Running
│   └── api_idle.json                # API → Idle
├── gemini/
│   └── poller_running.json
├── copilot/
│   └── poller_running.json
├── attention/
│   ├── completion_signal.json       # Idle + "completed" → InformationalCompleted
│   ├── admin_event.json             # wrapper-start → NO attention
│   └── error_attention.json         # Error → ActionRequiredError
├── edge_cases/
│   ├── expired_evidence.json        # TTL expired → ignore old evidence
│   ├── competing_evidence.json      # hook=Running + poller=Idle → Running wins
│   ├── empty_evidence.json          # no evidence → Unknown
│   └── state_precedence.json        # Error + Running → Error wins
└── scenarios/
    ├── claude_full_cycle.json        # Idle → Running → Approval → Running → Idle
    ├── claude_error_recovery.json    # Running → Error → Idle
    ├── codex_approval_flow.json      # Idle → Running → Approval → Running → Idle
    ├── poller_only.json              # hook なし、poller だけで state 検知
    ├── hook_to_poller_fallback.json  # hook 消失 → TTL 期限切れ → poller fallback
    └── state_flapping.json           # rapid Idle ↔ Running → stability check
```

### テスト実行

```rust
// agtmux-core/tests/fixtures.rs

#[test]
fn test_all_fixtures() {
    let fixtures = load_fixtures("../../fixtures");
    let engine = Engine::new(EngineConfig::default());

    for fixture in fixtures {
        let result = engine.resolve(&fixture.evidence, fixture.now);

        assert_eq!(result.state, fixture.expected.activity_state,
            "fixture '{}' failed: expected {:?}, got {:?}",
            fixture.name, fixture.expected.activity_state, result.state);

        assert!(result.confidence >= fixture.expected.confidence_min,
            "fixture '{}': confidence {:.2} below minimum {:.2}",
            fixture.name, result.confidence, fixture.expected.confidence_min);
    }
}
```

---

## 3. Layer 2: Replay Scenarios

### Scenario Format

```json
{
  "name": "claude_full_work_cycle",
  "description": "Claude Code: idle → running → approval → running → idle",
  "steps": [
    {
      "timestamp": "2026-02-22T10:00:00Z",
      "new_evidence": [
        {"provider": "claude", "signal": "Idle", "source": "Poller", "weight": 0.55, "confidence": 0.88, "ttl_secs": 90}
      ],
      "expected_activity": "Idle",
      "expected_attention": "None"
    },
    {
      "timestamp": "2026-02-22T10:00:05Z",
      "new_evidence": [
        {"provider": "claude", "signal": "Running", "source": "Hook", "weight": 0.95, "confidence": 0.92, "ttl_secs": 90}
      ],
      "expected_activity": "Running",
      "expected_attention": "None"
    },
    {
      "timestamp": "2026-02-22T10:00:30Z",
      "new_evidence": [
        {"provider": "claude", "signal": "WaitingApproval", "source": "Hook", "weight": 0.95, "confidence": 0.92, "ttl_secs": 90}
      ],
      "expected_activity": "WaitingApproval",
      "expected_attention": "ActionRequiredApproval"
    }
  ]
}
```

Replay test は evidence window を step ごとに蓄積し、TTL expiry を含めた時系列正しさをテストする。

---

## 4. Layer 3: Property-Based Tests (proptest)

```rust
use proptest::prelude::*;

proptest! {
    /// Empty evidence → Unknown
    #[test]
    fn empty_evidence_is_unknown(now in any_datetime()) {
        let engine = Engine::new(EngineConfig::default());
        let result = engine.resolve(&[], now);
        prop_assert_eq!(result.state, ActivityState::Unknown);
    }

    /// All evidence expired → Unknown
    #[test]
    fn expired_evidence_is_unknown(
        evidence in vec(any_evidence(), 1..10),
        delay_secs in 200u64..1000,
    ) {
        let engine = Engine::new(EngineConfig::default());
        let latest_ts = evidence.iter().map(|e| e.timestamp).max().unwrap();
        let now = latest_ts + Duration::from_secs(delay_secs);
        let result = engine.resolve(&evidence, now);
        prop_assert_eq!(result.state, ActivityState::Unknown);
    }

    /// Error evidence above threshold always wins
    #[test]
    fn error_always_wins(
        mut evidence in vec(any_evidence(), 1..5),
    ) {
        let engine = Engine::new(EngineConfig::default());
        let now = Utc::now();
        evidence.push(Evidence {
            signal: ActivityState::Error,
            weight: 1.0,
            confidence: 0.95,
            timestamp: now,
            ttl: Duration::from_secs(90),
            ..default_evidence()
        });
        let result = engine.resolve(&evidence, now);
        prop_assert_eq!(result.state, ActivityState::Error);
    }

    /// Score is always in [0.0, 1.0]
    #[test]
    fn score_bounded(evidence in vec(any_evidence(), 0..20)) {
        let engine = Engine::new(EngineConfig::default());
        let now = Utc::now();
        let result = engine.resolve(&evidence, now);
        prop_assert!(result.confidence >= 0.0);
        prop_assert!(result.confidence <= 1.0);
    }
}
```

**Go/Rust differential test は行わない** — 独自設計のため、fixture テストで精度を担保する。

---

## 5. Layer 4: Live Validation (Phase 3+)

### Recording Mode

```bash
$ agtmux daemon --record /tmp/agtmux-recording.jsonl
```

録画形式 (JSONL):

```jsonl
{"ts":"2026-02-22T10:00:00Z","type":"evidence","pane_id":"%1","evidence":[...]}
{"ts":"2026-02-22T10:00:00Z","type":"resolved","pane_id":"%1","state":"Running","confidence":0.92}
{"ts":"2026-02-22T10:00:00Z","type":"attention","pane_id":"%1","attention":"None"}
```

### Ground Truth Labeling

```bash
$ agtmux label /tmp/agtmux-recording.jsonl

# TUI でステップ実行
# j/k: 前後移動, r/w/a/i/e/u: running/waiting_input/approval/idle/error/unknown
```

出力 (ground truth file):

```jsonl
{"ts":"2026-02-22T10:00:00Z","pane_id":"%1","ground_truth":{"activity":"idle"}}
{"ts":"2026-02-22T10:00:05Z","pane_id":"%1","ground_truth":{"activity":"running"}}
```

### Accuracy Report

```bash
$ agtmux accuracy \
    --recording /tmp/recording.jsonl \
    --ground-truth /tmp/labels.jsonl

╔══════════════════════════════════════════════════════════╗
║  AGTMUX State Accuracy Report                            ║
╠══════════════════════════════════════════════════════════╣
║                                                          ║
║  Activity State                                          ║
║  ┌──────────────────┬───────────┬────────┬────────┐      ║
║  │ State            │ Precision │ Recall │ F1     │      ║
║  ├──────────────────┼───────────┼────────┼────────┤      ║
║  │ running          │ 0.95      │ 0.92   │ 0.93   │      ║
║  │ waiting_input    │ 0.88      │ 0.90   │ 0.89   │      ║
║  │ waiting_approval │ 0.97      │ 0.95   │ 0.96   │      ║
║  │ idle             │ 0.90      │ 0.88   │ 0.89   │      ║
║  │ error            │ 1.00      │ 1.00   │ 1.00   │      ║
║  ├──────────────────┼───────────┼────────┼────────┤      ║
║  │ Weighted F1      │           │        │ 0.92   │      ║
║  └──────────────────┴───────────┴────────┴────────┘      ║
║                                                          ║
║  Confusion Matrix (Activity)                             ║
║  ┌─────┬─────┬─────┬─────┬─────┬─────┐                  ║
║  │     │ run │ w_i │ w_a │ idl │ err │                  ║
║  │ run │ 42  │  1  │  0  │  2  │  0  │                  ║
║  │ w_i │  0  │ 18  │  1  │  1  │  0  │                  ║
║  │ w_a │  0  │  0  │ 12  │  0  │  0  │                  ║
║  │ idl │  3  │  0  │  0  │ 35  │  0  │                  ║
║  │ err │  0  │  0  │  0  │  0  │  5  │                  ║
║  └─────┴─────┴─────┴─────┴─────┴─────┘                  ║
║  (rows=ground truth, cols=predicted)                      ║
╚══════════════════════════════════════════════════════════╝
```

---

## 6. テスト改善ループ

```
1. Record — agtmux daemon --record session.jsonl
2. Label  — agtmux label session.jsonl
3. Measure — agtmux accuracy --recording ... --ground-truth ...
4. Diagnose — confusion matrix から弱点を特定
5. Fix — fixtures/ に問題ケースを追加、weight/confidence を調整
6. Verify — cargo test → 1 に戻る
```

### CI への組み込み

```yaml
# .github/workflows/accuracy.yml
- name: State Engine Tests
  run: |
    cargo test --test fixtures
    cargo test --test replay
    # proptest は cargo test で自動実行
```

`--gate dev` の場合:
- `activity_weighted_f1 >= 0.88` → pass
- それ未満 → CI fail

---

## 7. 実装優先順位

| 順序 | 項目 | Phase |
|------|------|-------|
| 1 | Unit fixture format + runner | Phase 0 |
| 2 | proptest invariant tests | Phase 0 |
| 3 | Replay scenario format + runner | Phase 0 |
| 4 | `agtmux daemon --record` | Phase 2 |
| 5 | `agtmux label` (TUI labeler) | Phase 4 |
| 6 | `agtmux accuracy` (report generator) | Phase 4 |
| 7 | CI accuracy gate | Phase 4 |
