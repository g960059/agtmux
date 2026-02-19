# Phase 19 E-F 実装ログ (2026-02-19)

## 実装範囲

- Phase 19-E: UI 判定ロジックの v2 優先切替
- Phase 19-F: daemon 側 observability 指標の追加

## 変更内容

### 1) Phase 19-E: App の v2 state 優先

- 更新: `macapp/Sources/Models.swift`
  - `PaneItem` に v2 フィールドを追加
    - `state_engine_version`
    - `provider_v2`, `provider_confidence_v2`
    - `activity_state_v2`, `activity_confidence_v2`, `activity_source_v2`
    - `activity_reasons_v2`
    - `evidence_trace_id`
- 更新: `macapp/Sources/AppViewModel.swift`
  - `activityState(for:)` は `v2-shadow` または `activity_state_v2` が存在する場合に v2 を最優先
  - `displayCategory(for:)` は v2 state を基準にカテゴリ導出
  - `stateReason(for:)` は `activity_reasons_v2` を優先表示
  - v2 未提供時のみ既存 fallback（legacy inference）を使用

### 2) Phase 19-F: daemon observability

- 更新: `internal/api/v1.go`
  - `ListSummary` に指標を追加
    - `v2_eval_count`
    - `v2_unknown_rate`
    - `v2_state_flip_rate`
    - `v2_decision_latency_ms`
    - `claude_title_match_rate`
    - `by_claude_title_source`
- 更新: `internal/daemon/server.go`
  - `buildPaneItems` で v2 評価時に上記指標を集計
  - pane ごとの v2 state 変化（flip）を記録する `recordV2StateFlip` を追加
  - Claude title source を分類して match rate を算出

## テスト

- 更新: `internal/daemon/server_test.go`
  - v2 shadow summary 指標の存在を検証
- 更新: `macapp/Tests/AppViewModelSettingsTests.swift`
  - v2 state が legacy 推定より優先されることを検証

## 実行結果

- `go test ./internal/daemon ./internal/stateengine ./internal/provideradapters ./internal/appclient ./cmd/agtmux-app` PASS
- `cd macapp && swift test` PASS

