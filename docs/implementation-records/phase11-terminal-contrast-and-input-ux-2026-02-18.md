# AGTMUX Phase 11 実装記録（Terminal Contrast/Input UX, 2026-02-18）

## Goal
- 内蔵TTYでの入力補助文（`Try ...` など）の視認性を改善する。

## 実装
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - ANSI 正規化段に `applyBlackForegroundContrastFix` を追加。
  - `SGR 30`（黒前景）かつ背景指定なしの文字列に対し、淡い背景色（`48;2;218;226;236`）を付与。
  - 目的: 黒前景文字がガラス背景上で潰れる問題を抑止。

## 検証
- `cd macapp && swift test` PASS
- アプリ再ビルド/再起動済み
