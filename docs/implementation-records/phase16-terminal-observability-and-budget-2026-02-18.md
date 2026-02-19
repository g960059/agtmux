# AGTMUX Phase 16 実装記録（Terminal Observability + Performance Budget, 2026-02-18）

## Goal
- 内蔵TTYの体感性能（描画/入力/stream）を定量的に観測できる状態にする。
- "遅い" を感覚ではなく予算（budget）で判定し、次フェーズの最適化判断をしやすくする。

## 変更点

### 1. Terminal telemetry スナップショットを導入
- `macapp/Sources/AppViewModel.swift`
  - `TerminalPerformanceSnapshot` を追加。
    - `renderFPS`
    - `inputLatencyP50Ms`
    - `streamRTTP50Ms`
    - `inputSampleCount`
    - `streamSampleCount`
  - `@Published private(set) var terminalPerformance` を追加。

### 2. 計測パイプラインを追加
- `macapp/Sources/AppViewModel.swift`
  - render frame 計測:
    - `noteTerminalFrameRendered`
    - `frameRenderTimestamps`
  - input→frame 遅延計測:
    - `noteTerminalInputDispatched`
    - `noteTerminalFrameApplied`
    - `pendingInputLatencyStartByPaneID`
  - stream RTT 計測:
    - `noteTerminalStreamRoundTrip`
    - `streamLatencySamplesMs`
  - 低オーバーヘッドの集計ヘルパー:
    - `trimFrameRenderWindow`
    - `appendLatencySample`
    - `percentile`
    - `recomputeTerminalPerformanceSnapshot`

### 3. budget 判定を追加
- `macapp/Sources/AppViewModel.swift`
  - 予算:
    - input p50 <= 120ms
    - stream p50 <= 220ms
    - render fps >= 24
  - `terminalPerformanceWithinBudget` を追加。
  - `terminalPerformanceSummary`（`fps / input / stream`）を追加。

### 4. 実フローへ telemetry を接続
- `macapp/Sources/AppViewModel.swift`
  - interactive write 開始時に input dispatch を記録。
  - frame 適用時 (`applyTerminalFrame` / `applyTerminalStreamFrame`) に input latency 完了を記録。
  - `terminalStream` / `terminalRead` の往復で stream RTT を記録。
  - pane 切替/kill/prune/cancel で pending 計測データをクリアし、古い pane のノイズを除去。

### 5. render callback を terminal view へ追加
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - `onFrameRendered` callback を追加。
  - `terminal.feed(text:)` の直後に callback を発火。
- `macapp/Sources/AGTMUXDesktopApp.swift`
  - `onFrameRendered` を `AppViewModel.noteTerminalFrameRendered` へ接続。

### 6. UI 表示（技術詳細ON時）
- `macapp/Sources/AGTMUXDesktopApp.swift`
  - sidebar footer に telemetry summary を追加。
  - budget 内/外で色分け（通常/attention）。

## テスト（RED -> GREEN）
- 追加:
  - `macapp/Tests/AppViewModelSettingsTests.swift`
    - `testTerminalPerformanceCollectsInputStreamAndFPSMetrics`
    - `testTerminalPerformanceBudgetFailsWhenLatencyAndFPSArePoor`

## 検証
- `cd macapp && swift test --filter AppViewModelTerminalProxyTests/testEnqueueInteractiveInputBatchesRapidBytesIntoSingleWrite` PASS
- `cd macapp && swift test` PASS
- `go test ./... -count=1` PASS

## 期待効果
- 「重い/遅い」を `fps / input p50 / stream p50` で追跡可能。
- 次フェーズの最適化（描画更新粒度・stream間隔・I/Oバッチ）を数値で比較可能。
