# AGTMUX Phase 10 実装記録（Terminal Smoothness, 2026-02-18）

## 背景
- Codex/Claude の `Working` 表示（点滅 + shimmer）が内蔵TTY上でカクつく。
- 原因は、短周期更新時でも毎回「全量split + 全画面再描画」を行っていたこと。

## 変更概要
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - 描画パスを最適化:
    - `content` 差分がない更新は **cursor-only 更新**（カーソル移動シーケンスのみ）へ分岐。
    - `content` 更新時は行キャッシュを更新し、毎回の全量 `split` を回避。
    - pane切替/detach時にキャッシュを明示リセット。
  - 結果: 不要なフルリペイントを削減し、`Working` アニメーションの体感FPSを改善。

- `macapp/Sources/AppViewModel.swift`
  - stream polling cadence を高速化:
    - fast: `90ms -> 70ms`
    - normal: `180ms -> 120ms`
    - idle: `280ms -> 220ms`
  - stream line budget を軽量化:
    - default: `160 -> 120`
    - min: `120 -> 90`
    - max: `320 -> 240`
    - baseline: `rows + 48 -> rows + 32`

- `internal/daemon/server.go`
  - stream capture の最小間隔を短縮:
    - `120ms -> 80ms`
  - 目的: client 側 cadence 引き上げ時の「cache-only応答」割合を下げる。

## テスト
- `cd macapp && swift test` PASS
- `go test ./internal/daemon ./internal/appclient` PASS

## 実行確認
- アプリ再ビルド + 再起動済み
  - `macapp/scripts/install-app.sh`
  - `open ~/Applications/AGTMUXDesktop.app`

## 次の確認ポイント
- Codex `Working` 行の shimmer と dot blink の滑らかさ（体感）
- CPU使用率（長時間稼働時）
- 大量出力paneでのスクロール保持挙動
