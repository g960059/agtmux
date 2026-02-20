# Phase 20 実装ログ (2026-02-20)

## 実施範囲

- Phase 20 設計 (`tmux-first`, attention 再定義, session last-active) の初期実装。
- 画面切替としての `By Status` を廃止し、status filter lens に移行。
- `task_completed` を actionable queue から分離し informational queue に移行。

## 変更点

### 1) API / daemon

- `internal/api/v1.go`
  - `PaneItem` 追加:
    - `attention_state`
    - `attention_reason`
    - `attention_since`
    - `session_last_active_at`
    - `session_time_source`
    - `session_time_confidence`
  - `ListSummary` 追加:
    - `by_attention_state`
    - `actionable_attention_count`
    - `informational_count`
    - `session_time_known_rate`
    - `session_time_match_rate`
    - `session_time_unknown_rate`

- `internal/daemon/server.go`
  - `derivePaneSessionActiveTime()` を追加し、managed pane の session 時刻を provider 優先で解決。
  - `deriveAttentionState()` を追加し、`action_required_*` / `informational_completed` を算出。
  - `buildPaneItems()` で新フィールドを返却。
  - summary に attention/state-time 指標を集計。

### 2) stateengine 型

- `internal/stateengine/types.go`
  - attention 定数群（`Attention*`）を追加。
  - `SessionTime` 型を追加（今後の resolver 拡張用）。

### 3) mac app（model / ViewModel / UI）

- `macapp/Sources/Models.swift`
  - API 新フィールドの decode 対応。

- `macapp/Sources/AppViewModel.swift`
  - `StatusFilter` を導入（`All/Attention/Running/Idle/Unmanaged/Unknown`）。
  - `filteredPanes` に status filter を適用し、session tree を維持。
  - `paneRecencyDate` を managed + `session_last_active_at` 優先に変更。
  - `attentionState(for:)` を実装し、`displayCategory` / `needsUserAction` に反映。
  - queue を分離:
    - `reviewQueue` = actionable
    - `informationalQueue` = completion/info
  - `observeTransitions()` を attention-state ベースへ変更。

- `macapp/Sources/AGTMUXDesktopApp.swift`
  - サイドバーを session tree 固定化（tmux-first）。
  - status filter chips を追加。
  - session header に actionable attention バッジ (`A<n>`) を追加。

### 4) テスト

- 既存 test を維持したまま全 pass。
- `macapp/Tests/AppViewModelSettingsTests.swift` に追加:
  - managed pane の session-last-active 表示
  - actionable attention で category が attention になること
  - status filter が attention pane に絞り込むこと

### 5) CLI 集計整合

- `cmd/agtmux-app/main.go`
  - `summarizePanes()` を拡張し、daemon 追加項目に追従:
    - `by_category`
    - `by_attention_state`
    - `actionable_attention_count`
    - `informational_count`
    - `session_time_known/match/unknown_rate`

## 実行結果

- `go test ./internal/daemon ./internal/stateengine ./cmd/agtmux-app` PASS
- `cd macapp && swift test` PASS (88 tests)

## 次スライス

- informational queue の専用 UI（Inbox）を追加。
- `attention_resolution_seconds` など運用指標の可視化を footer/settings に追加。
- provider 別 session time resolver の confidence 校正（特に claude fallback 条件）。
