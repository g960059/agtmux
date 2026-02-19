# AGTMUX Phase 15 実装記録（Multi-target Ops Hardening, 2026-02-18）

## Goal
- multi-target 運用時に、target 障害の影響を sidebar 操作へ波及させにくくする。
- down/degraded な SSH target の復旧を自動化し、手動オペレーションを減らす。

## 変更点

### 1. target health を session 並びに反映
- `macapp/Sources/AppViewModel.swift`
  - `sessionSections` sort に target metadata を追加。
  - 優先順:
    1. pinned session
    2. default target
    3. target health（`ok` > `degraded` > `down` > `unknown`）
    4. session sort mode（stable/recent/name）

### 2. SSH target 自動再接続（指数バックオフ）
- `macapp/Sources/AppViewModel.swift`
  - `autoReconnectTargetsIfNeeded` を追加。
  - 対象: `kind=ssh` かつ `health != ok` かつ `connection_ref` 有効。
  - バックオフ: 4s -> 8s -> 16s ... 最大 90s。
  - reconnect 成功時に state をクリアし、target health を `ok` として即時反映。
  - sweep/in-flight ガードで同時多重実行を抑止。

### 3. 手動再接続導線を追加
- `macapp/Sources/AGTMUXDesktopApp.swift`
  - session context menu に `Reconnect Target` を追加（ssh target のみ）。
  - session header の target ラベルに health dot を追加。
    - `ok`: running color
    - `degraded`: attention color
    - `down`: error color

## テスト（RED -> GREEN）
- `macapp/Tests/AppViewModelSettingsTests.swift` に追加:
  - `testSessionSortPrefersDefaultTargetThenHealth`
  - `testAutoReconnectAttemptsDownSSHTargetAndMarksHealthOnSuccess`
  - `testAutoReconnectBackoffSkipsRapidRetriesAfterFailure`

## 検証
- `cd macapp && swift test --filter AppViewModelSettingsTests` PASS
- `cd macapp && swift test` PASS
- `go test ./... -count=1` PASS

## 補足
- `swift test` 1回目で `AppViewModelTerminalProxyTests` に既存のタイミング依存タイムアウトが1件発生したため、個別再実行後に全体再実行し PASS を確認。
