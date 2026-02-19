# AGTMUX Phase 13 実装記録（Pane Switch Terminal Cache, 2026-02-18）

## Goal
- pane 選択切替時に main terminal を即時表示し、stream 応答待ちの空白時間を減らす。
- 内蔵TTYを第一導線として使う際の体感レスポンスを改善する。

## 変更点

### 1. pane 単位の terminal render cache を導入
- `macapp/Sources/AppViewModel.swift`
  - `TerminalRenderCache` を追加（output/cursor/size）。
  - `terminalRenderCacheByPaneID` を追加。

### 2. pane 選択時の即時復元
- `selectedPane.didSet` で:
  - 新規 pane に cache があれば `outputPreview` と cursor/size を即時復元。
  - cache がない場合のみクリア表示へ。

### 3. frame 適用時に cache 更新
- `applyTerminalFrame` / `applyTerminalStreamFrame` 後に cache 更新。
- snapshot 反映時に存在しない pane の cache を prune。

## テスト（RED -> GREEN）
- 追加:
  - `macapp/Tests/AppViewModelTerminalProxyTests.swift`
  - `testPaneReselectRestoresCachedTerminalPreviewWithoutStreaming`
    - auto-stream off でも reselect で即時復元できることを確認。
    - reselect 自体では追加 stream が発生しないことを確認。

## 検証
- `cd macapp && swift test --filter AppViewModelTerminalProxyTests` PASS
- `cd macapp && swift test` PASS

## 期待効果
- pane 切替時の「一瞬真っ白」体験を低減。
- stream latency に依存しない初期表示が可能になり、操作連続性が向上。
