# AGTMUX Phase 7 実装記録・意思決定ログ（2026-02-15）

関連記録:
- `docs/implementation-records/phase1-6-implementation-decision-log-2026-02-15.md`

## 1. 目的とスコープ

本フェーズでは、以下を主目的として実装を継続した。

1. `Codex` pane の会話セッション情報（label / last interaction）を精度高く表示する。
2. `tmux` 由来の汎用情報より、agent会話由来の情報を優先する。
3. mac app から daemon 運用操作（Refresh/Restart 表示）を隠し、通常UXをシンプル化する。
4. 障害時は自動復旧中心にしつつ、最小限の手動復旧導線を残す。
5. 実装後に 2 本の Codex レビューを実施し、採否を明示する。

## 2. 実装過程（時系列）

### Step 1: 既存差分の確認と未実装箇所の特定

- 未配線だった領域を確認:
  - daemon 側: `codex app-server` 取得情報の `buildPaneItems` 反映
  - mac app 側: daemon操作UIの整理、自動復旧フロー
- 既存差分の整合性確認:
  - `current_path` 収集・永続化（tmux observer / DB migration / store）は導入済みであることを確認

### Step 2: daemon 側に Codex thread ヒントを配線

実装内容:

- `Server` に `codexSessionEnricher` を保持
  - `internal/daemon/server.go`
- `buildPaneItems` で Codex 対象 pane の `current_path` を収集し、`GetMany` で path 単位にヒント取得
  - `internal/daemon/server.go`
- `derivePaneSessionLabel` で `codex_thread_list` を最優先ソースとして採用
  - `internal/daemon/server.go`
- `derivePaneLastInteractionAt` で Codex hint timestamp を反映
  - `internal/daemon/server.go`
- managed pane で信頼できる会話シグナルが無い場合に、tmux の `last_activity_at/updated_at` を安易に採用しない方針へ変更
  - `internal/daemon/server.go`

### Step 3: Codex app-server parser の互換性を改善

実装内容:

- `thread/list` のレスポンスで `result.data` だけでなく `result.threads` も読めるよう改善
  - `internal/daemon/codex_appserver.go`
- その後、レビュー指摘を受けて `data + threads` を統合し、最新時刻を優先してヒントを選ぶロジックへ強化
  - `internal/daemon/codex_appserver.go`

### Step 4: mac app UI/復旧フローの整理

実装内容:

- ヘッダーから常時表示の daemon 操作を削除:
  - `Daemon Running` pill
  - `Refresh`
  - `Restart Daemon`
  - 対象: `macapp/Sources/AGTMUXDesktopApp.swift`
- `refresh` 失敗時の自動復旧を実装:
  - `daemon.ensureRunning(with: client)` を条件付き実行
  - cooldown 付きで多重実行を抑制
  - 対象: `macapp/Sources/AppViewModel.swift`
- `bootstrap` 初回失敗でも polling を継続し、後続回復を許容
  - `macapp/Sources/AppViewModel.swift`
- Status view の session metadata 表示をデフォルト false に変更（UI情報量削減）
  - `macapp/Sources/AppViewModel.swift`
- 追加対応（レビュー採用）:
  - daemon エラー時のみ `Retry` ボタンを表示（通常時は隠す）
  - `macapp/Sources/AGTMUXDesktopApp.swift`

### Step 5: テスト追加と再検証

追加/更新したテスト:

- `internal/daemon/presentation_test.go`
  - codex hint 優先ラベル
  - managed pane の fallback 抑制
  - unmanaged pane fallback
  - codex hint timestamp 適用
- `internal/daemon/codex_appserver_test.go`
  - `data` / `threads` 混在時に最新ヒントを選ぶケース

実行した検証:

- `go test ./...` pass
- `cd macapp && swift test` pass
- `cd macapp && swift build` pass

## 3. 意思決定ログ（採用した判断）

### D-13: Codex会話情報は `path -> thread/list` で補完する

- 背景:
  - pane 名や tmux の静的情報だけでは、agent 会話セッション識別の精度が不足していた。
- 採用:
  - `codex app-server` の `thread/list` から preview/timestamp を取得し、pane 表示へ補完する。

### D-14: managed pane の `last active` は「信頼シグナル優先」

- 背景:
  - tmux 更新時刻をそのまま採用すると「常に数秒前」の誤表示が起きる。
- 採用:
  - runtime入力/イベント/Codexヒントがある場合のみ更新し、無い場合は空表示を許容する。

### D-15: daemon運用操作は「通常非表示 + 異常時のみ露出」

- 背景:
  - ユーザーに daemon/restart/refresh を常時意識させない方針。
- 採用:
  - 平常時は自動復旧に任せる。
  - 異常時のみ `Retry` を表示して手動導線を残す。

## 4. 2 Codex Review の結果と採否

## Review A（実装品質）

主な指摘:

1. `data/threads` 混在時の重複・競合リスク（High）
2. 自動復旧の過剰発火リスク（High）
3. managed pane fallback 抑制による時刻欠損リスク（High）

採否:

1. 採用
   - parser を `data + threads` 統合 + 最新時刻優先に修正
   - テスト追加
2. 一部採用済みとして据え置き
   - 既に `RuntimeError` 種別で復旧発火を制限しており、今回の追加修正は見送り
3. 非採用（仕様判断）
   - 「誤った fresh 表示」を避けるため、managed pane の安易な時刻補完は行わない

## Review B（UI/UX/DX）

主な指摘:

1. 自動復旧失敗時の手動導線不足（High）
2. 復旧状態可視化不足（Medium）
3. metadata 非表示既定で診断導線が弱くなる可能性（Medium）

採否:

1. 採用
   - daemon error 時のみ `Retry` を表示
2. 継続課題として保留
   - 復旧中表示/再試行カウント等の可視化は次フェーズ候補
3. 仕様維持
   - 初期表示のシンプルさを優先し、設定トグルで補う方針を維持

## 5. 変更ファイルマップ（本フェーズ）

- `internal/daemon/server.go`
  - codexEnricher 配線
  - buildPaneItems への Codex hint 反映
  - session label / last interaction 生成ロジック更新
- `internal/daemon/codex_appserver.go`
  - thread/list parser の `data/threads` 互換化
  - 最新時刻優先ロジック
- `internal/daemon/codex_appserver_test.go`
  - `data/threads` 混在ケース追加
- `internal/daemon/presentation_test.go`
  - presentation ロジックのユニットテスト追加
- `macapp/Sources/AppViewModel.swift`
  - 自動復旧フロー
  - polling 継続
  - UI default 設定更新
- `macapp/Sources/AGTMUXDesktopApp.swift`
  - daemon操作UI削除
  - エラー時のみ `Retry` 表示

## 6. 残課題（次フェーズ候補）

1. 自動復旧状態の可視化（`recovering`, `next retry in`, `last success`）
2. `thread/list` データ品質が低い場合のフォールバック品質ルール（短文/空文/古いpreview）
3. UIテレメトリ:
   - 自動復旧成功率
   - 復旧時間（MTTR）
   - `Retry` 使用率

## 7. 実行コマンド（再現用）

```bash
# Go
gofmt -w internal/daemon/server.go internal/daemon/codex_appserver.go internal/daemon/codex_appserver_test.go internal/daemon/presentation_test.go
go test ./...

# mac app
cd macapp && swift test
cd macapp && swift build
```

