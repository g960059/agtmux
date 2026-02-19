# AGTMUX Phase 14 実装記録（Sidebar Scalability: Pinning + Pinned Filter, 2026-02-18）

## Goal
- session 数が増えても sidebar の操作性を維持する。
- 重要 session を固定表示できるようにし、一覧の並び変化による誤操作を減らす。

## 変更点

### 1. Session Pinning を追加
- `macapp/Sources/AppViewModel.swift`
  - pin 永続化キーを追加:
    - `ui.pinned_sessions`
    - `ui.show_pinned_only`
  - `pinnedSessionKeys: Set<String>` を導入。
  - API を追加:
    - `isSessionPinned(target:sessionName:)`
    - `setSessionPinned(target:sessionName:pinned:)`
    - `toggleSessionPinned(target:sessionName:)`
  - snapshot 適用時に存在しない session pin を prune。
  - session kill 時に pin も同時削除。

### 2. Session Sort に pin 優先を追加
- `sessionSections` で sort 前段に `pinned` 優先ロジックを追加。
- sort mode（stable/lastActive/name）に関わらず pin session が先頭側に集約される。

### 3. Pinned Only filter を追加
- `@Published showPinnedOnly` を追加し UserDefaults に永続化。
- ON 時は pin された session だけ sidebar に表示。

### 4. UI 統合
- `macapp/Sources/AGTMUXDesktopApp.swift`
  - sort/filter popover に `Pinned Only` toggle を追加。
  - session header に pin アイコン表示。
  - session context menu に `Pin Session` / `Unpin Session` を追加。

## テスト（RED -> GREEN）
- 追加:
  - `macapp/Tests/AppViewModelSettingsTests.swift`
    - `testPinnedSessionSortsBeforeUnpinnedRegardlessOfSortMode`
    - `testPinnedSessionsPersistAcrossModelInstances`
    - `testShowPinnedOnlyFiltersSessionSections`

## 検証
- `cd macapp && swift test --filter AppViewModelSettingsTests` PASS
- `cd macapp && swift test` PASS
- `go test ./... -count=1` PASS

## 期待効果
- session が多い環境でも「重要 session を常に上位固定」できる。
- sort mode を変えても重要 session が見失われにくくなる。
- 一時的に pin 対象だけに絞って作業集中できる。
