# AGTMUX Phase 1-6 実装記録・意思決定ログ

Status: Active (PoC)
Last Updated: 2026-02-15
Scope: `exp/go-codex-implementation-poc`

## 0. 保存先ポリシー（`docs` と `artifacts` の使い分け）

結論:

- 恒久的に参照する設計・実装判断・運用方針は `docs/` に置く。
- レビュー生ログ、検証中の一時メモ、手元で生成した中間アウトプットは `artifacts/`（または `/tmp`）に置く。

本書は「このブランチで採用した判断を後から再利用する」ことが目的なので `docs/` 配置とする。

推奨ディレクトリ規約:

- `docs/implementation-records/`: 採用済み判断を含む実装記録（本書）
- `artifacts/reviews/`: 外部レビューの生ログ（timestamp付き）
- `artifacts/test-runs/`: 長時間テスト出力・検証ログ

## 1. 背景と目的

tmux を agent 実行の永続化基盤として使い、`agtmuxd` を単一の状態ソースにして、CLI と mac app の両方から同じ状態/操作を扱う。

今回の実装主眼:

1. event-first で provider/status 推定精度を上げる
2. idempotency/replay の正しさを担保する
3. mac app を session-pane-first の運用に寄せる
4. install 時の導入負荷を下げる（hooks/notify/wrapper）

## 2. 実装サマリ（ここまで）

### 2.1 基本コンポーネント

- `agtmux`（CLI）
- `agtmuxd`（daemon, UDS HTTP API）
- `agtmux-app`（resident loop / app bridge CLI）
- `AGTMUXDesktop`（SwiftUI mac app, daemon 自動起動/再起動）

主要参照:

- `cmd/agtmux`
- `cmd/agtmuxd`
- `cmd/agtmux-app`
- `internal/daemon`
- `internal/ingest`
- `macapp/Sources`

### 2.2 状態モデル

- canonical state: `running / waiting_input / waiting_approval / completed / idle / error / unknown`
- UI 表示は `agent_presence` と `activity_state` を軸に、`display_category`（attention/running/idle/unmanaged/unknown）へ集約

参照:

- `docs/macapp-ui-state-model.md`
- `internal/daemon/server.go`
- `macapp/Sources/AppViewModel.swift`

### 2.3 event-driven 連携

- `POST /v1/events` ingest（`hook|notify|wrapper|poller`）
- `integration install` で Claude hooks / Codex notify / wrapper を導入
- `integration doctor` で導入状態を検査

参照:

- `docs/event-driven-integration.md`
- `internal/integration/install.go`
- `internal/integration/install_test.go`

## 3. 意思決定ログ（採用した判断）

## D-01: status 推定は「LLM推論」ではなく「イベント契約 + ルール」で実装する

- 決定:
  - 推定精度改善に OpenRouter 等の低価格 LLM を挟まない。
  - hot path は deterministic なルールベースで維持する。
- 理由:
  - コスト、遅延、再現性、障害時のデバッグ性で不利。
  - 状態遷移は監査可能性が重要。
- トレードオフ:
  - 柔軟性より安定性を優先。

## D-02: Codex は notify + wrapper + poller fallback の多層で追跡する

- 決定:
  - Codex の公式 hook を前提にせず、現行は notify/wrapper で event を補完。
- 理由:
  - 現実の CLI 変化に対して、単一手段依存は壊れやすい。
- 実装:
  - `internal/integration/install.go`
  - `~/.codex/config.toml` の notify 設定管理

## D-03: install は idempotent + backup + atomic write を必須にする

- 決定:
  - 設定変更は再実行安全にし、既存設定破壊を避ける。
- 実装:
  - 既存 notify がある場合はデフォルトで上書きしない
  - `--force-codex-notify` で明示置換
- 参照:
  - `internal/integration/install.go`
  - `internal/integration/install_test.go`

## D-04: Session-Pane first UI を正式採用し、window は任意メタにする

- 決定:
  - 主要構造は `target > session > pane`。
  - `window` は `off|auto|on` 設定で補助表示。
- 理由:
  - 実運用（1 worktree = 1 session, 1 pane = 1 agent）に整合。
- 実装:
  - `macapp/Sources/AppViewModel.swift` (`ViewMode`, `WindowGrouping`)
  - `docs/macapp-ui-state-model.md`

## D-05: `completed` は主表示の長期状態ではなく、review queue のイベントとして扱う

- 決定:
  - 完了は「キューに残る確認項目」として保持。
  - toast 依存で取りこぼさない。
- 実装:
  - `ReviewQueueItem` と dedupe 制御
  - `task_completed`, `needs_input`, `needs_approval`, `error` の queue kind
- 参照:
  - `macapp/Sources/AppViewModel.swift`

## D-06: poller は補助信号とし、event-driven state を TTL で保護する

- 決定:
  - event-driven 更新直後は poller で上書きさせない。
  - `ttl <= 0` は「抑制しない」にする（安全側）。
- 実装:
  - `internal/ingest/engine.go` `shouldSuppressPollerByEventDrivenState`
  - 基準時刻は `LastSeenAt` を採用

## D-07: future `event_time` は skew budget で clamp する

- 決定:
  - 未来時刻イベントで TTL/順序が壊れないよう clamp。
- 実装:
  - `internal/ingest/engine.go` `clampEventTime`
  - `internal/daemon/server.go` ingest request の event_time clamp

## D-08: idempotency key conflict は厳格に reject する

- 決定:
  - 同一 `dedupe_key` でも event の同一性が崩れる再送は conflict。
- 実装:
  - `internal/ingest/engine.go` `isReplayCompatible`
  - `model.ErrIdempotencyConflict` 返却

## D-09: duplicate replay は「順序」と「状態推定」を分離する

- 決定:
  - 順序復元: stored event を基準にする
  - 状態推定: retry payload 優先（payload 由来分類を維持）
- 理由:
  - 保存済み event payload が redaction で空の場合があるため
- 実装:
  - `internal/ingest/engine.go`
    - `mergeReplayStateEvent`
    - `replayPayloadHintsConflict`
- 関連指摘:
  - re-review blocking BI-1 に対する修正

## D-10: payload 保存は fail-closed redaction

- 決定:
  - 安全に redact できない payload は保存しない。
- 実装:
  - `internal/security/redaction.go` `RedactForStorage`
- トレードオフ:
  - 後段の payload 依存分析は欠損し得る
  - ただしセキュリティを優先

## D-11: migration rollback は DB 再生成方式を運用標準にする

- 決定:
  - SQLite の環境差分を避けるため、additive migration の rollback は DB 再生成で対応。
- 実装/文書化:
  - `internal/db/migrations.go` v2 DownSQL コメント
  - `README.md` `Database Rollback`

## D-12: mac app は self-contained bundle を採用する

- 決定:
  - `.app` に `agtmuxd` / `agtmux-app` を同梱。
  - app 起動時に daemon health を見て必要なら起動。
- 実装:
  - `macapp/scripts/package-app.sh`
  - `macapp/scripts/install-app.sh`
  - `macapp/Sources/CommandRuntime.swift`

## 4. 今回の re-review 対応（重要）

対象レビュー:

- `/tmp/agtmux-phase1-6-review-v2.md`
- `/tmp/agtmux_phase1_6_rereview_20260215.md`

### 4.1 解消した blocking

- BI-1: duplicate replay 時に stored payload 欠損で state 誤復元

対応内容:

1. duplicate replay 分岐で retry payload 優先の state 推定へ変更
2. payload ヒント不整合チェックを追加（双方 payload がある場合のみ）
3. P0 テスト追加（MT-1〜MT-3）

### 4.2 追加したテスト

- `internal/ingest/engine_test.go`
  - `TestIngestDuplicateReplayPreservesPayloadDerivedState`
  - `TestIngestPartialFailureThenReplayPayloadDependentClassification`
- `internal/daemon/events_ingest_test.go`
  - `TestEventsAPI_IdempotentRetryWithDifferentEventIDAndEventTime`

## 5. 実装マップ（変更密度の高い領域）

### 5.1 Ingest / State Engine

- `internal/ingest/engine.go`
- `internal/ingest/engine_test.go`

内容:

- runtime stale guard
- source cursor ordering
- dedupe/idempotency
- poller suppression
- replay repair
- payload-derived classification

### 5.2 Adapter / Normalization

- `internal/adapter/codex.go`
- `internal/adapter/codex_test.go`

内容:

- JSON semantic key 優先の notify hint 抽出
- false positive（`error`/`failed` 裸一致）抑制
- approval/input/completed/error の優先順序調整

### 5.3 Integration Install

- `internal/integration/install.go`
- `internal/integration/install_test.go`

内容:

- codex notify 設定注入
- 既存 notify の安全扱い
- force replace
- wrapper/helper 配置

### 5.4 Daemon/API

- `internal/daemon/server.go`
- `internal/daemon/events_ingest_test.go`

内容:

- `/v1/events` ingest request validation
- future event_time clamp
- runtime/pane bind
- idempotent retry behavior

### 5.5 DB / Migration

- `internal/db/store.go`
- `internal/db/migrations.go`

内容:

- event dedupe lookup API（`GetEventByRuntimeSourceDedupe`）
- states provenance fields
- rollback運用の明示

### 5.6 mac app

- `macapp/Sources/AGTMUXDesktopApp.swift`
- `macapp/Sources/CommandRuntime.swift`
- `macapp/Sources/AppViewModel.swift`
- `macapp/scripts/run-dev.sh`
- `macapp/scripts/package-app.sh`
- `macapp/scripts/install-app.sh`

内容:

- daemon 自動起動/再起動
- session/status 2モード表示
- review queue
- send/view-output/kill 操作

## 6. 検証ログ（現時点）

実施コマンド:

```bash
go test ./...
cd macapp && swift build
```

結果:

- Go テスト: pass
- mac app build: pass

起動確認:

- `~/Applications/AGTMUXDesktop.app` から起動
- 同梱 `agtmuxd` / `agtmux-app` で疎通可能

## 7. 残課題（非 blocking）

代表例:

- shell wrapper と Go adapter の判定ロジック重複（将来乖離リスク）
- `valuesContain` の部分一致で厳密一致化余地
- event/cursor/state を単一 transaction に統合する余地
- review queue を backend 永続化 API に昇格する余地

## 8. 次に進めるときのガイド

1. 仕様/方針変更がある場合は `docs/agtmux-spec.md` と本書を同時更新する。
2. UI 状態モデル変更は `docs/macapp-ui-state-model.md` を先に更新する。
3. re-review の生ログは `artifacts/reviews/` に timestamp 付きで保存し、本書には要約のみ追記する。
4. gate 判定は「blocking 0件 + `go test ./...` + `swift build`」を最小条件にする。

## 9. 付録: よく使うコマンド

```bash
# daemon
./bin/agtmuxd

# integration
./bin/agtmux integration install
./bin/agtmux integration doctor --json

# event emit
./bin/agtmux event emit --target local --pane %1 --agent codex --source wrapper --type wrapper-start

# list/watch
./bin/agtmux list panes --json
./bin/agtmux watch --scope panes --json --once

# mac app build/install
./macapp/scripts/package-app.sh
./macapp/scripts/install-app.sh
open ~/Applications/AGTMUXDesktop.app
```
