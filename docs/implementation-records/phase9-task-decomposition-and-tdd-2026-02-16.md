# AGTMUX Phase 9 タスク分解 + TDD実行計画（2026-02-16）

対象戦略:
- `docs/implementation-records/phase9-main-interactive-tty-strategy-2026-02-16.md`

## 1. 方針

Phase 9 は次の順で進める。

1. Step 0 gate（技術選定 Spike）を完了
2. daemon streaming endpoint + daemon proxy PTY を実装
3. macapp の永続接続クライアントを実装
4. interactive terminal UI を段階導入（feature flag）
5. 回復/降格/fallback を実装
6. AC をテストで閉じる

TDDの原則:
- 各タスクを **RED -> GREEN -> REFACTOR** で進める
- REDは必ず「自動テストの失敗」で確認する
- GREEN後に重複除去・命名改善・責務分離を実施する
- `Manual+CI` の項目は RED ではなく「補助検証」として扱う

## 2. タスク分解（実行順）

| Task ID | Step | Priority | Task | Depends On | First RED Tests | Exit |
| --- | --- | --- | --- | --- | --- | --- |
| TASK-901 | 0 | P0 | streaming endpoint 契約ドラフト作成（attach/detach/write/resize/stream） | - | TC-901 | API契約草案がレビュー通過 |
| TASK-902 | 0 | P0 | daemon proxy PTY local PoC | TASK-901 | TC-902, TC-903 | key-by-key と output stream の成立 |
| TASK-903 | 0 | P0 | terminal emulator PoC（SwiftTerm優先） | TASK-902 | TC-902 | key/IME/resize の成立可否を判断 |
| TASK-904 | 0 | P0 | SLO計測PoC（入力遅延/切替遅延） | TASK-902 | TC-916 | NFR見込みを数値で確認 |
| TASK-911 | 1 | P0 | `TerminalSessionManager` 実装（daemon） | TASK-904 | TC-902, TC-915 | attach/detach lifecycle と GC成立 |
| TASK-912 | 1 | P0 | `/v1/terminal/attach|detach|write|resize` 実装 | TASK-911 | TC-902, TC-903, TC-907 | API契約準拠 + stale guard |
| TASK-913 | 1 | P0 | `/v1/terminal/stream` 実装（長寿命ストリーム） | TASK-912 | TC-902, TC-905 | stream frame 契約を満たす |
| TASK-921 | 2 | P0 | macapp 永続UDSクライアント実装 | TASK-913 | TC-902 | CLI都度起動なしで接続可能 |
| TASK-922 | 2 | P0 | `TerminalSessionController` 実装 | TASK-921 | TC-904, TC-905 | attach/retry/degrade制御が成立 |
| TASK-923 | 2 | P1 | AppViewModel 統合（write系 stale guard 経路統一） | TASK-922 | TC-907 | 誤pane送信を拒否できる |
| TASK-924 | 2 | P0 | resize競合ポリシー実装（tmux複数client前提） | TASK-923 | TC-909 | resize時の競合挙動が仕様どおり |
| TASK-931 | 3 | P0 | interactive terminal view 追加（既存UI並存） | TASK-924 | TC-904 | feature flagで切替可能 |
| TASK-932 | 3 | P1 | fallback導線実装（Open in External Terminal） | TASK-931 | TC-917 | 常時 fallback 実行可能 |
| TASK-941 | 4 | P0 | terminal recovery（指数バックオフ + 自動降格） | TASK-932 | TC-905 | 連続失敗時に確実に降格 |
| TASK-942 | 4 | P1 | kill switch 実装（interactive即無効化） | TASK-941 | TC-914 | Snapshot既定に戻せる |
| TASK-951 | 5 | P0 | daemon contract/integration テスト拡充 | TASK-942 | TC-902, TC-903, TC-907, TC-915 | Goテストで契約固定 |
| TASK-952 | 5 | P0 | macapp unit/UI テスト拡充 | TASK-951 | TC-904, TC-905, TC-917 | Swiftテストで回帰防止 |
| TASK-953 | 5 | P1 | perf/leak テスト実装 | TASK-952 | TC-910, TC-911, TC-912 | NFR判定を自動化 |
| TASK-961 | 6 | P0 | local先行リリース | TASK-953 | TC-911, TC-912 | localでAC達成 |
| TASK-962 | 6 | P1 | ssh段階拡張 | TASK-961 | TC-913 | sshで退行なし |

## 3. RED->GREEN->REFACTOR 具体ループ

| フェーズ | 目的 | 実施内容 |
| --- | --- | --- |
| RED | 失敗を先に固定 | 新規テストを追加し、現実装で失敗を確認 |
| GREEN | 最小実装で通す | 仕様を満たす最短実装のみ投入 |
| REFACTOR | 保守性改善 | 重複除去、責務分離、命名改善、ログ整備 |

適用ルール:
- 1タスクあたり最大3テストから開始
- GREENで通るまで新機能追加をしない
- REFACTOR後に必ず全関連テストを再実行

## 4. 先に作るREDテスト（優先順）

1. TC-902: `terminal/attach + stream` 契約
2. TC-903: key-by-key write round-trip
3. TC-907: write系 stale guard 拒否
4. TC-916: Step0 SLO feasibility smoke
5. TC-904: 50ms間隔100回 pane切替の一致性
6. TC-905: session establish/stream/write 失敗時の自動降格

補助検証（RED対象外）:
- TC-906: 外部terminal fallback UX
- TC-908: IME/修飾キー 実機確認
- TC-913: ssh parity 実機確認

## 5. テスト実行コマンド

Go:
```bash
go test ./internal/daemon -run 'Terminal|Attach|Stream|Stale' -count=1
go test ./... -count=1
```

Swift:
```bash
cd macapp && swift test --filter Terminal
cd macapp && swift test
```

統合確認:
```bash
go test ./... && (cd macapp && swift test)
```

## 6. Gate判定

Step 0 gate を通過する条件:
- TC-901, TC-902, TC-903, TC-916 が期待どおり
- NFR-1/NFR-2 の見込み値（PoC計測）が提示される
- 方式選定（endpoint + proxy PTY）がレビュー承認される

Phase 9 実装開始条件:
- Step 0 gate pass
- Critical/High の未解決レビュー指摘がない

## 7. 実行ログ（2026-02-16）

### 7.1 Step1-CacheTTL（TASK-911 の一部）

- 目的:
  - daemon 内 terminal cache (`terminalProxy`, `terminalStates`) の寿命管理を追加し、長時間運用時のリークリスクを抑える
- RED:
  - `TestTerminalStreamRejectsExpiredProxySession`
  - `TestTerminalReadResetsWhenCachedStateExpired`
  - 追加直後は `terminalProxyTTL` / `terminalStateTTL` 未実装で build fail を確認
- GREEN:
  - `internal/daemon/server.go` に以下を実装
    - `defaultTerminalStateTTL`, `defaultTerminalProxySessionTTL`
    - `Server.terminalStateTTL`, `Server.terminalProxyTTL`
    - `terminalReadState.updatedAt`
    - `pruneExpiredTerminalCaches(now)` helper
    - `terminal/attach|detach|write|stream|read` 経路で prune 実行
  - `terminal/read` 更新時に `updatedAt` を保存
- REFACTOR:
  - `gofmt` 適用
  - prune ロジックを helper に集約
- 検証:
  - `go test ./internal/daemon -run 'TestTerminal(StreamRejectsExpiredProxySession|ReadResetsWhenCachedStateExpired)' -count=1` PASS
  - `go test ./internal/daemon -count=1` PASS
  - `go test ./... -count=1` PASS
  - `cd macapp && swift test` PASS

備考:
- `terminalProxy` は `UpdatedAt`（fallback: `CreatedAt`）基準で期限切れ判定。
- `terminalStates` は `updatedAt` 基準で期限切れ判定。

### 7.2 Test Harness Hardening（macapp）

- 目的:
  - Swift テストの `setenv` グローバル依存を減らし、並列実行時の相互干渉リスクを下げる
- 変更:
  - `AGTMUXCLIClient` に `init(socketPath:appBinaryPath:)` を追加
  - `DaemonManager` に `daemonBinaryPath` 注入オプションを追加
  - `AppViewModelSettingsTests` / `AppViewModelTerminalProxyTests` を明示注入へ移行
- 検証:
  - `cd macapp && swift test --filter AppViewModelSettingsTests` PASS
  - `cd macapp && swift test --filter AppViewModelTerminalProxyTests` PASS
  - `cd macapp && swift test` PASS

### 7.3 TASK-922 Step（TerminalSessionController 導入）

- 目的:
  - `AppViewModel` に散在していた terminal session/cursor/failure 管理を controller に集約し、pane 単位の retry/degrade/recovery を明確化する
- RED:
  - `macapp/Tests/TerminalSessionControllerTests.swift` を追加
  - 初回実行で `TerminalSessionController` 未定義 build fail を確認
- GREEN:
  - `macapp/Sources/TerminalSessionController.swift` を追加
    - pane 単位の `proxySession` / `cursor` lifecycle
    - 連続失敗時の指数バックオフ
    - failure threshold 到達時の degrade（cooldown 付き）
    - cooldown 後の proxy 再許可
  - `macapp/Sources/AppViewModel.swift` を更新
    - send/view-output/stream-loop を controller 経由に変更
    - stale session の detach と pane switch cleanup を controller 経由に変更
  - `macapp/Sources/CommandRuntime.swift` を修正
    - `commandRunner` default 互換修正
    - Unix socket connect 時の `sockaddr_un` path copy を安定化
- REFACTOR:
  - `applySnapshot` の stale session cleanup を単一路に整理
- 追加テスト:
  - `testFailureThresholdEntersDegradedModeAndRecoversAfterCooldown`
  - `testRecordSuccessClearsFailuresAndDegradedState`
  - `testPaneSessionAndCursorLifecycle`
  - `testRetryDelayIsExponentiallyBackedOffAndCapped`
  - `testPerformViewOutputDegradesToTerminalReadAfterProxyFailures`
- 検証:
  - `cd macapp && swift test --filter TerminalSessionControllerTests` PASS
  - `cd macapp && swift test --filter AppViewModelTerminalProxyTests` PASS
  - `cd macapp && swift test --filter AppViewModelSettingsTests` PASS
  - `cd macapp && swift test` PASS
  - `go test ./internal/daemon -run 'TestTerminal(StreamRejectsExpiredProxySession|ReadResetsWhenCachedStateExpired)' -count=1` PASS
  - `go test ./internal/daemon -count=1` PASS
  - `go test ./... -count=1` PASS

### 7.4 TASK-923 Step（write系 stale guard 経路統一）

- 目的:
  - write系操作（`action send` / `action kill` / `terminal attach`）で stale guard の指定経路を統一し、誤pane送信・古い状態への操作を防ぎやすくする
  - pane切替時の cursor clear と stream開始順序を修正し、stale cursor 送信レースを防ぐ
- RED:
  - `AppViewModelTerminalProxyTests` に以下を追加
    - `testAutoAttachIncludesRuntimeAndStateGuards`
    - `testPaneReselectClearsCursorBeforeFirstStreamRequest`
  - `CommandRuntimeTests` に以下を追加
    - `testSendTextIncludesStaleGuardFlagsWhenProvided`
    - `testKillIncludesStaleGuardFlagsWhenProvided`
  - 追加直後は stale guard 未付与・cursor順序未保証で失敗を確認
- GREEN:
  - `macapp/Sources/CommandRuntime.swift`
    - `sendText` / `kill` に `ifRuntime/ifState/ifUpdatedWithin/forceStale` を追加
  - `macapp/Sources/AppViewModel.swift`
    - `writeGuardOptions(for:)` を追加
    - `performSend` の snapshot path と `kill` path に guard を配線
    - `kill` は stale guard conflict 時に `forceStale=true` で1回だけ再試行（運用性確保）
    - `ensureTerminalProxySession` の `terminalAttach` へ guard を配線
    - pane切替時の処理を整理し、`autoStreamOnSelection=true` のときは cursor clear 完了後に stream 再開
    - `performSend` がローカル変数 `text` を使うよう修正（入力競合回避）
  - `macapp/Tests/AppViewModelTerminalProxyTests.swift`
    - attach session を pane依存 (`term-<pane>`) にし、pane別 stream 判定を安定化
- REFACTOR:
  - `CommandRuntimeTests` の引数捕捉を `ArgsCapture`（lock付き）に置換し、Swift concurrency warning を解消
  - proxy系テストの待機タイムアウトを調整し、フルスイート時の不安定失敗を低減
- 検証:
  - `cd macapp && swift test --filter CommandRuntimeTests` PASS
  - `cd macapp && swift test --filter AppViewModelTerminalProxyTests` PASS
  - `cd macapp && swift test` PASS（単独実行）
  - `go test ./... -count=1` PASS

備考:
- `cd macapp && swift test` と `go test ./...` の並列実行では、proxy系テストがタイムアウトしやすいため、最終判定は単独実行結果を採用。

### 7.5 TASK-924 Step（resize競合ポリシー）

- 目的:
  - tmux 複数 client 環境で `terminal/resize` が外部 client と競合しないように、明示ポリシーを実装する
- RED:
  - `TestTerminalResizeSkipsWhenMultipleClientsAttached` を追加
  - 追加直後は resize が常に実行されるため失敗することを確認
- GREEN:
  - `internal/daemon/server.go`
    - `terminalResizeHandler` に client 数判定を追加
    - `tmux list-clients -t <session> -F #{client_tty}` 実行
    - `client_count > 1` なら `result_code=skipped_conflict` で no-op
    - `client_count <= 1` なら `resize-pane` 実行
    - client 検査失敗時は fail-safe で `result_code=skipped_conflict`（`inspection_fallback_skip`）
    - 判定情報を返すため `policy/client_count/reason` を response に追加
  - `internal/api/v1.go` / `macapp/Sources/Models.swift`
    - `TerminalResizeResponse` に `policy`, `client_count`, `reason` を追加
  - `internal/daemon/server_test.go`
    - 既存 `TestTerminalReadAndResizeHandlers` を client 判定前提に更新
    - `TestTerminalResizeSkipsWhenMultipleClientsAttached` を追加
    - `TestTerminalResizeSkipsWhenClientInspectionFails` を追加
    - `TestTerminalResizeAppliesWhenNoTmuxClientAttached` を追加
- REFACTOR:
  - pane 解決処理を `resolvePaneByTargetIDPaneID` helper に分離
  - `tmuxClientCount` helper を分離
- 検証:
  - `go test ./internal/daemon -run 'TestTerminal(ReadAndResizeHandlers|ResizeSkipsWhenMultipleClientsAttached|ResizeSkipsWhenClientInspectionFails|ResizeAppliesWhenNoTmuxClientAttached)' -count=1` PASS
  - `go test ./... -count=1` PASS
  - `cd macapp && swift test` PASS

### 7.6 TASK-931 Step（interactive input 導入 + feature flag）

- 目的:
  - 既存 `send/read` UI を残したまま、選択 pane に対して key-by-key / text-chunk 入力経路を導入する
  - interactive 経路を UI 設定で on/off できるようにし、段階導入可能にする
- RED:
  - `macapp/Tests/AppViewModelTerminalProxyTests.swift` に以下を追加
    - `testPerformInteractiveInputSendsTerminalKey`
    - `testPerformInteractiveInputSendsTerminalTextChunk`
  - 追加直後は `performInteractiveInput` 未定義で build fail を確認
- GREEN:
  - `macapp/Sources/AppViewModel.swift`
    - `performInteractiveInput(text:key:)` を追加
    - proxy経路で `terminal/write -> terminal/stream` を即時実行
    - proxy非対応時の text フォールバックと key 非対応エラーを追加
    - 設定項目 `interactiveTerminalInputEnabled` を追加し `UserDefaults` 永続化
  - `macapp/Sources/TerminalInputCaptureView.swift` を追加
    - `NSViewRepresentable` ベースの key capture 層
    - Enter / Tab / Backspace / Escape / Arrow / Home / End / PageUp/Down と Ctrl/Alt 修飾を tmux key へ変換
  - `macapp/Sources/AGTMUXDesktopApp.swift`
    - terminal表示領域に `TerminalInputCaptureView` を配線
    - settings menu に `Enable Interactive Key Input` toggle を追加
- REFACTOR:
  - interactive capture の focus 制御を `focusToken` 差分時のみ再フォーカスへ制限
- 追加テスト:
  - `macapp/Tests/AppViewModelSettingsTests.swift`
    - `testInteractiveTerminalInputPreferenceDefaultsToTrueAndPersists`
- 検証:
  - `cd macapp && swift test --filter AppViewModelSettingsTests` PASS
  - `cd macapp && swift test --filter AppViewModelTerminalProxyTests/testPerformInteractiveInput` PASS
  - `go test ./internal/daemon -run 'TestTerminal(ReadAndResizeHandlers|ResizeSkipsWhenMultipleClientsAttached|ResizeSkipsWhenClientInspectionFails|ResizeAppliesWhenNoTmuxClientAttached)' -count=1` PASS

### 7.7 TASK-932 Step（Open in External Terminal fallback 導線）

- 目的:
  - interactive terminal が失敗/不適合な状況でも、選択 pane を外部 Terminal で即時に開ける fallback を常時提供する
  - local / ssh target の両方で、pane 指定付き tmux jump を 1 action で実行できるようにする
- RED:
  - `macapp/Tests/AppViewModelSettingsTests.swift` に以下を追加
    - `testOpenSelectedPaneInExternalTerminalBuildsLocalTmuxJumpCommand`
    - `testOpenSelectedPaneInExternalTerminalBuildsSSHCommand`
  - 追加直後は `openSelectedPaneInExternalTerminal` と command 生成経路が未実装で失敗することを確認
- GREEN:
  - `macapp/Sources/Models.swift`
    - `TargetItem` に `connectionRef`（`connection_ref`）を追加
  - `macapp/Sources/AppViewModel.swift`
    - `openSelectedPaneInExternalTerminal()` を追加
    - local 用 `tmux select-window/select-pane/attach-session` command 生成を追加
    - ssh target では `connection_ref` を使って `ssh -t <ref> '<tmux-command>'` を構築
    - `osascript` で Terminal.app `do script` 実行する launcher を追加
    - runner 注入ポイント `externalTerminalCommandRunner` を追加（テスト容易化）
  - `macapp/Sources/AGTMUXDesktopApp.swift`
    - terminal header と pane context menu に `Open in External Terminal` action を追加
- REFACTOR:
  - shell/AppleScript 文字列エスケープ helper を `AppViewModel` に集約
  - 実行 runner を注入可能にして、UI からの副作用を unit test で安全に検証できる形へ整理
- 追加テスト:
  - `ExternalTerminalRunCapture` を追加し、実行 command を lock 付きで捕捉
  - `makeModel` helper に `externalTerminalCommandRunner` 注入を追加
  - `testOpenSelectedPaneInExternalTerminalFailsWhenTargetIsUnavailable` を追加
  - `testOpenSelectedPaneInExternalTerminalFailsWhenTargetKindIsUnsupported` を追加
  - `testOpenSelectedPaneInExternalTerminalFailsWhenTargetKindIsUnavailable` を追加
  - `testOpenSelectedPaneInExternalTerminalFailsWhenSSHConnectionRefMissing` を追加
  - `testOpenSelectedPaneInExternalTerminalFailsWhenPaneIdentityIncomplete` を追加
  - `testOpenSelectedPaneInExternalTerminalRequiresSelection` を追加
  - `testOpenSelectedPaneInExternalTerminalClearsInfoMessageOnRunnerFailure` を追加
- レビュー反映（2026-02-16）:
  - 2 Codex review を実施
    - general: `/tmp/agtmux_task932_review_general_result_20260216.md`
    - general-rerun: `/tmp/agtmux_task932_review_general_rerun3_result_20260216.md`
    - ui/ux: `/tmp/agtmux_task932_review_uiux_result_20260216.md`
  - 採用:
    - `attach-session` 先行実行を廃止し、pane 指定確実化のため `select-window/select-pane` を先に実行
    - target 未解決時の暗黙 `local` フォールバックを廃止し、`target is unavailable` で fail-fast
    - target kind の fail-open を廃止し、`local|ssh` 以外は明示エラー
    - tmux jump command の連結を `;` から `&&` に変更し、途中失敗時に attach を中止
  - 判定:
    - general 初回は `Stop`、修正後 rerun は `Go with changes`
    - ui/ux は `Go with changes`
    - 1/2 gate: pass（実際は 2/2 pass）
  - 見送り:
    - UI文言/ショートカット/アクセシビリティ改善は別UI phaseで実施（今回の P1 fallback 契約範囲外）
- 検証:
  - `cd macapp && swift test --filter AppViewModelSettingsTests/testOpenSelectedPaneInExternalTerminal` PASS
  - `cd macapp && swift test --filter AppViewModelSettingsTests/testOpenSelectedPaneInExternalTerminalBuildsSSHCommand` PASS
  - `go test ./... -count=1` PASS
  - `cd macapp && swift test` PASS

### 7.8 TASK-953 Step（Snapshot Poll 負荷削減: daemon aggregated snapshot）

- 目的:
  - `AppViewModel.refresh()` が 1 回につき `agtmux-app view targets/sessions/windows/panes` を4プロセス起動していた経路を削減し、CPU/遅延を下げる
  - 同一時点の `targets/sessions/windows/panes` を 1 API 応答で取得し、UI 反映の整合性を上げる
- RED:
  - `internal/daemon/server_test.go`
    - `TestSnapshotEndpointAggregatesViews`
    - `TestSnapshotEndpointMethodNotAllowed`
  - `macapp/Tests/CommandRuntimeTests.swift`
    - `testFetchSnapshotUsesDaemonTransportWhenAvailable`
    - `testFetchSnapshotFallsBackToCLIWhenDaemonTransportUnavailable`
  - 追加直後は `/v1/snapshot` と transport API が未実装で失敗することを確認
- GREEN:
  - `internal/api/v1.go`
    - `DashboardSnapshotEnvelope` を追加
  - `internal/daemon/server.go`
    - `GET /v1/snapshot` を追加
    - target filter を踏襲し、`targets/sessions/windows/panes` を1レスポンスへ集約
  - `macapp/Sources/Models.swift`
    - `DashboardSnapshotEnvelope` を追加
  - `macapp/Sources/CommandRuntime.swift`
    - `TerminalDaemonTransport` に `dashboardSnapshot(target:)` を追加
    - `DaemonUnixTerminalTransport` に `/v1/snapshot` 実装
    - `AGTMUXCLIClient.fetchSnapshot()` を daemon snapshot 優先へ切替
    - daemon unavailable 時のみ既存CLI fallback（4 view command）を使用
- REFACTOR:
  - snapshot 取得を `withDaemonFallback` 経路へ統一し、他 terminal API と同一の失敗ポリシーへ整理
- 検証:
  - `go test ./internal/daemon -run 'TestSnapshotEndpoint|TestListEndpointsFilterSummaryAndAggregation' -count=1` PASS
  - `go test ./... -count=1` PASS
  - `cd macapp && swift test --filter CommandRuntimeTests` PASS
  - `cd macapp && swift test` PASS
  - `cd macapp && ./scripts/install-app.sh` 実施後、アプリ再起動確認済み

### 7.9 TASK-953 Step（Snapshot fallback 方針の厳格化）

- 目的:
  - daemon 不調時に `fetchSnapshot()` が CLI 4プロセス fallback を連発する経路を止め、UI 遅延/CPUスパイクを抑える
  - 後方互換（旧daemon）だけは維持するため、`/v1/snapshot` が 404 の場合のみ CLI fallback を許可する
- RED:
  - `macapp/Tests/CommandRuntimeTests.swift`
    - `testFetchSnapshotDoesNotFallbackToCLIWhenDaemonTransportUnavailable`
    - `testFetchSnapshotFallsBackToCLIWhenSnapshotEndpointIsNotFound`
- GREEN:
  - `macapp/Sources/CommandRuntime.swift`
    - `fetchSnapshot()` を専用分岐へ変更
      - daemon transport 成功: `/v1/snapshot` を採用
      - daemon transport 404(`/v1/snapshot`): CLI fallback
      - それ以外の daemon エラー: 即エラー返却（fallbackしない）
    - CLI fallback 経路を `fetchSnapshotViaCLI()` に分離
- REFACTOR:
  - snapshot だけ fallback ポリシーを明示化し、他APIと混在した暗黙 fallback を回避
- 検証:
  - `cd macapp && swift test --filter CommandRuntimeTests` PASS
  - `cd macapp && swift test` PASS
  - `cd macapp && ./scripts/install-app.sh` 後、`/v1/snapshot` 応答と app/daemon 起動を確認済み
