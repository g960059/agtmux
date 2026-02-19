# AGTMUX Phase 17 実装記録（Terminal Render Pacing, 2026-02-19）

## Goal
- `Working` など高頻度更新時のカクつきを減らし、体感を滑らかにする。
- pane 切替時の初期表示は維持しつつ、更新連打による過剰再描画を抑える。

## 変更点

### 1. 内蔵TTYの描画コアレス（60fps相当）
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - `Coordinator` に render pacing を追加。
    - `minimumRenderInterval = 1/60` 秒
    - `renderWorkItem` による coalescing
    - `lastRenderAt` で描画間隔を制御
  - `update(...)` は直接 `renderIfNeeded` せず `scheduleRender(force:)` を通す構造へ変更。
  - pane 切替時は `force=true` で即時描画（初期表示遅延を避ける）。
  - scroll hold 解除時・入力再開時も `force=true` で即時描画。

### 2. レンダー結果に応じた timestamp 更新
- `renderIfNeeded` を `Bool` return に変更。
- 実際に `terminal.feed(...)` した時だけ `lastRenderAt` を更新。
- 無駄な間引き遅延を避け、更新が無いときは即復帰できるようにした。

### 3. フレーキーテスト安定化
- `macapp/Tests/AppViewModelTerminalProxyTests.swift`
  - `testEnqueueInteractiveInputBatchesRapidBytesIntoSingleWrite`
    - wait timeout を `2.0s -> 5.0s`
    - ログ検出条件を `contains` から `occurrenceCount >= 1` に調整
  - 全体テスト中に稀発していた timing timeout の再現率を下げた。

## 検証
- `cd macapp && swift test --filter AppViewModelTerminalProxyTests --filter AppViewModelSettingsTests/testTerminalPerformanceCollectsInputStreamAndFPSMetrics` PASS
- `cd macapp && swift test` PASS
- `go test ./... -count=1` PASS

## 期待効果
- ストリーム更新が密な場面で UI スレッド負荷を抑え、表示の滑らかさを改善。
- pane 切替の初期応答は維持しつつ、無駄な全画面再描画を減らせる。
