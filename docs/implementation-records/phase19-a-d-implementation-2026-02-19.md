# Phase 19 A-D 実装ログ (2026-02-19)

## 実装範囲

- Phase 19-A: `stateengine` 土台
- Phase 19-B: Claude adapter
- Phase 19-C: Codex adapter
- Phase 19-D: Gemini/Copilot adapter（最小実装）

## 変更概要

### 1. state engine v2 (shadow mode)

- 追加: `internal/stateengine/`
  - `types.go`: evidence / pane meta / evaluation / adapter interface
  - `fsm.go`: state 優先順位と閾値判定
  - `engine.go`: provider 検出 + evidence 合成 + explainable 出力
- 方針:
  - `unknown + recent` だけで running に上げない
  - unmanaged は `provider=none` として分離
  - 評価結果は trace id を付与

### 2. provider adapter 分離

- 追加: `internal/provideradapters/`
  - `registry.go`: adapter registry
  - `claude.go`, `codex.go`, `gemini.go`, `copilot.go`
  - `helpers.go`
- 効果:
  - provider ごとの差分ロジックを daemon 本体から分離
  - Phase 19-D までの adapter 拡張を core 変更なしで追加

### 3. daemon への shadow 接続

- 更新: `internal/daemon/server.go`
  - `Server` に `stateEngine` を追加
  - `NewServerWithDeps` で `stateengine + provideradapters.DefaultRegistry()` を構築
  - `buildPaneItems` 内で v2 評価を実施（既存 v1 判定は維持）
- API 追加（shadow 可観測化）:
  - `PaneItem` に `*_v2` と `evidence_trace_id` を追加
  - `ListSummary` に `by_state_v2`, `by_provider_v2`, `by_source_v2` を追加

## テスト

- 追加:
  - `internal/stateengine/engine_test.go`
  - `internal/provideradapters/registry_test.go`
  - `internal/daemon/server_test.go` に v2 shadow 項目検証
- 実行結果:
  - `go test ./...` PASS
  - `cd macapp && swift test` PASS

## ビルド/起動確認

- `macapp/scripts/install-app.sh` 実行済み
- `AGTMUXDesktop` / `agtmuxd` 再起動済み
- launcher log で `ensureRunning: healthy` を確認

