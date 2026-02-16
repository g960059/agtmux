# AGTMUX Phase 9 設計記録 v3（Main Embedded Interactive TTY, 2026-02-16）

Status: Draft v3 (Step 0 gate before implementation)

統合元:
- v2: `docs/implementation-records/phase9-main-interactive-tty-strategy-2026-02-16.md`
- v3: `docs/implementation-records/phase9-main-interactive-tty-strategy-v3-2026-02-16.md`（統合後に削除）
- task decomposition: `docs/implementation-records/phase9-task-decomposition-and-tdd-2026-02-16.md`

## 1. 要約

Phase 9 は `main` を「send/read UI」から「interactive terminal UI」へ移行する。  
採用方式は以下の2層構成とする。

1. **daemon streaming endpoint**（外部契約）
2. **daemon proxy PTY**（内部実装）

この構成により、`local`/`ssh` を同一APIで扱い、multi-target 運用を維持したまま key-by-key 対話を実現する。

## 2. 背景と課題

現行 main 画面は `terminal read + send` 前提で、以下が不足している。

- slash command (`/skills` `/resume`) の実用性
- shell 補完/履歴/修飾キー/IME
- pane選択直後の即作業導線

また、監視は daemon、操作は疑似TTYという分離により、状態と操作の不整合リスクがある。

## 3. 意思決定

### 3.1 採用
- **採用**: `daemon streaming endpoint + daemon proxy PTY`

### 3.2 非採用
- app 直PTYを主系にする案  
: local では成立しても multi-target/ssh/運用統制で破綻しやすい
- chat-first案  
: CLI互換追従コストが高く、tmux中心運用とズレる

## 4. Goal / Non-goal

### Goal
- pane選択で app 内から tmux pane を双方向操作できる
- `local`/`ssh` を同一UXで扱える
- 状態監視（sidebar）と実作業（main）を1画面で完結

### Non-goal
- tmux 置換
- Codex/Claude 独自 chat UI 実装
- daemon/API の破壊的変更

## 5. 要件（更新版）

### 5.1 Functional

- FR-1: pane選択で terminal attach セッション開始
- FR-2: key-by-key 入力（Ctrl/Alt/Escape/F-key/矢印）
- FR-3: slash command を app 内実行可能
- FR-4: shell 補完/履歴/IME を阻害しない
- FR-5: 高速切替時も選択paneと表示paneが一致
- FR-6: resize の競合方針を定義し一貫動作
- FR-7: terminal session 回復（再試行/降格）
- FR-8: fallback（SnapshotBackend + 外部terminal）常設
- FR-9: write系に stale runtime/state guard 必須

### 5.2 Non-functional

- NFR-1: local 入力反映 p95 <= 150ms
- NFR-2: pane切替同期 p95 <= 500ms
- NFR-3: 8時間連続稼働で session leak 閾値内
- NFR-4: daemon不調時も UI 非ハング

### 5.3 測定定義

- M-1: key event発火 -> echo表示
- M-2: pane選択更新 -> 正しいstream初回描画
- M-3: attach/detach反復時のメモリ/FD増分

## 6. アーキテクチャ

### 6.1 外部契約: daemon streaming endpoint

追加API（案）:
- `POST /v1/terminal/attach`
- `POST /v1/terminal/detach`
- `POST /v1/terminal/write`
- `POST /v1/terminal/resize`
- `GET /v1/terminal/stream`（long-lived stream over UDS）

`/v1/terminal/stream` フレーム（案）:
- `attached`
- `output`
- `input-ack`（任意）
- `resize-ack`
- `error`
- `detached`

### 6.2 内部実装: daemon proxy PTY

- daemon内に `TerminalSessionManager` を追加
- targetごとに proxy PTY セッションを管理
- local/ssh の差分を daemon 側で吸収
- app は target 差分を意識せず同一操作

### 6.3 app 側

- `TerminalSessionController` を導入
- backend:
  - `InteractiveTTYBackend`（streaming 主系）
  - `SnapshotBackend`（既存 `terminal/read` 副系）
- `feature flag` で main 表示を段階切替

## 7. レビュー指摘への対応

### Critical への対応

1. 接続プロトコル未定義  
: streaming endpoint を契約として先に定義
2. terminal emulator 未選定  
: Step 0 で PoC 比較（SwiftTerm優先）
3. daemon API進化計画不在  
: attach/write/stream API を追加する前提に変更

### High への対応

- 既存UIの即廃止をやめる（段階移行）
- fallback 表示先を維持（SnapshotBackend継続）
- write系 stale guard 必須化
- fallback導線（Open in External Terminal）先行実装

## 8. 実装計画

### Step 0: Spike/Gate（必須）

- S0-1: endpoint/proxy PTY の最小PoC（local）
- S0-2: terminal emulator PoC（key/IME/resize）
- S0-3: SLO試算（NFR-1達成見込み）
- Gate: Critical 未解決なら Stop

### Step 1: daemon 基盤

- `TerminalSessionManager` 実装
- `/v1/terminal/*` endpoint 実装
- `terminalStates` TTL/GC 導入

### Step 2: macapp 接続層

- 永続UDSクライアント（CLI都度起動経路を主系から除外）
- `TerminalSessionController` 実装

### Step 3: UI 段階統合

- interactive terminal view を追加
- 既存 snapshot/send UI は残す
- feature flag で切替

### Step 4: Recovery/Fallback

- terminal session 回復（指数バックオフ）
- 失敗閾値で SnapshotBackend 降格
- 外部terminal起動導線

### Step 5: Test/Perf

- unit/integration/ui/perf を実装
- AC 判定を自動化

### Step 6: Rollout

- local target 先行
- ssh target を段階拡張

## 9. Acceptance Criteria（更新）

- AC-1: Codex/Claude pane で slash command 実行可能
- AC-2: zsh 補完/履歴/IME が成立
- AC-3: 50ms間隔100回切替で不一致0件
- AC-4: session establish/stream/write の失敗時に再試行または降格が機能
- AC-5: stale guard が write系全操作で有効
- AC-6: fallback 導線が常時利用可能
- AC-7: `swift test` / `go test ./...` green

## 10. リスクと対策

1. terminal emulator成熟度
- 対策: Step 0 PoCと代替案評価

2. resize競合（複数クライアント）
- 対策: tmux設定前提を明文化し統合テスト化

3. ssh遅延揺らぎ
- 対策: local/ssh でSLO分離評価

4. state cache増大
- 対策: TTL/GC + metrics

## 11. 実装開始条件

以下が揃うまで実装本体に入らない。

- Step 0 レポート（方式選定、PoC結果、SLO見込み）
- APIドラフト（attach/write/stream）
- terminal emulator 選定理由
- fallback/kill switch 運用手順
