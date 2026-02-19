# AGTMUX Phase 12 実装記録（Status Inference Hardening, 2026-02-18）

## Goal
- `running/idle/attention/unmanaged` 判定で poller 由来の誤判定（特に `running` 過判定）を減らす。
- event/hook/wrapper の情報を優先し、poller は補助信号として扱う。

## 変更点

### 1. poller fallback の厳格化
- `internal/daemon/server.go`
  - `derivePaneLastInteractionAt` を修正。
  - `state_source=poller` の場合、`lastActivityAt` を managed pane に使わない。
  - `lastActivityAt` fallback は unmanaged pane のみに限定。

### 2. 状態昇格/降格ロジックの強化
- `internal/daemon/server.go`
  - `refinePanePresentationWithSignals` に `stateSource` 引数を追加。
  - `idle/unknown -> running` 昇格は以下を満たす場合のみ:
    - managed
    - poller source
    - recent interaction
    - running hint が reason/event に存在
  - `running -> idle` 降格を追加:
    - managed
    - poller source
    - running hint 不在
    - interaction stale

### 3. running/idle/attention ヒント判定を分離
- `internal/daemon/server.go`
  - `hasRunningHint`
  - `hasIdleOrCompletionHint`
  - `hasIdleLikeHint`
  - `hasAttentionHint`
  - `normalizeSignalToken`

## テスト（RED -> GREEN）
- 更新:
  - `internal/daemon/presentation_test.go`
    - managed poller fallback の期待値を `nil` に変更
  - `internal/daemon/server_test.go`
    - `refinePanePresentationWithSignals` の引数更新
- 追加:
  - `TestRefinePanePresentationWithSignalsDoesNotPromotePollerIdleWithoutRunningSignal`
  - `TestRefinePanePresentationWithSignalsPromotesPollerIdleWithRunningSignal`
  - `TestRefinePanePresentationWithSignalsDemotesStalePollerRunningWithoutSignal`

## 検証
- `go test ./internal/daemon -count=1` PASS
- `go test ./... -count=1` PASS

## 期待効果
- poller ノイズによる `running` 過判定を抑制。
- managed pane の activity 表示が event/hook 主体に寄るため、実態と整合しやすくなる。
