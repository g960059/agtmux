# AGTMUX Phase 18 実装記録（TTY Scrollback + Stream Rate Tuning, 2026-02-19）

## Goal
- 内蔵TTYで wheel scroll が実用的に動くようにする。
- codex/claude 実行中の低fps体感（5-7fps）を改善する。

## 変更点

### 1. stream poll interval を高速化
- `macapp/Sources/AppViewModel.swift`
  - `terminalStreamPollFastMillis`: `70 -> 45`
  - `terminalStreamPollNormalMillis`: `120 -> 75`
  - `terminalStreamPollIdleMillis`: `220 -> 140`

### 2. stream line budget を増加
- `macapp/Sources/AppViewModel.swift`
  - `terminalStreamDefaultLines`: `120 -> 240`
  - `terminalStreamMinLines`: `90 -> 160`
  - `terminalStreamMaxLines`: `240 -> 1200`
  - baseline: `rows + 32 -> rows + 200`

### 3. full repaint を「1画面固定」から「履歴保持」へ変更
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - `buildAbsoluteRepaintFrame` の行正規化ロジックを刷新。
  - これまで: `sourceRows` に合わせて terminalRows へ切り詰め。
  - 今回: 履歴行（複数画面分）を保持し、再描画時に terminal 側 scrollback を維持。
  - カーソル位置は pane viewport の `cursorY` を基準に、履歴全体への絶対行へ再マップ。

### 4. cursor-only repaint も同じ座標系に統一
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - `buildCursorOnlyFrame` でも履歴基準の cursor mapping を採用。
  - 旧 `mapCursorPosition` を削除。

## Why fps 5-6 が起こるか
- 表示している `fps` は GPU の描画上限ではなく、**terminal frame が実際に適用された頻度**。
- stream poll / frame 供給が遅い、または差分が少ない場合、fps は 5-7 付近になる。
- `input 35ms / stream 7ms` は I/O 自体は良好で、ボトルネックは主に frame cadence 側。

## 検証
- `cd macapp && swift test --filter AppViewModelTerminalProxyTests --filter AppViewModelSettingsTests` PASS
- `go test ./... -count=1` PASS

