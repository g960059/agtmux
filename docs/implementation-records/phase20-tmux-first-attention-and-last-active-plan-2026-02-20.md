# Phase 20: Tmux-First Attention / Last Active 設計計画 (2026-02-20)

## 1. 背景

現行は `by session / by status` の表示軸が混在し、`tmux first` の操作導線と `attention` の意味がぶれやすい。
また、`last active` は pane 活動時刻寄りで、`codex/claude` の会話セッション時刻（`/resume` 相当）と必ずしも一致しない。

## 2. 方針（意思決定）

1. 主ナビゲーションは **by session 固定**（tmux 構造を崩さない）。
2. by status は画面切替ではなく **filter lens**（絞り込み）に降格。
3. `attention` は状態ではなく **ユーザー介入要求**のシグナルとして定義。
4. `last active` は agent pane のみ表示し、**agent session 時刻**を優先。

## 3. ゴール / 非ゴール

### ゴール

- `tmux session -> pane` 文脈を維持したまま、attention pane を即座に見つけられる。
- `last active` が `codex/claude` の会話セッション最終時刻に高精度で一致する。
- `zsh/unmanaged` pane の時刻ノイズを排除する。
- `task_completed` を actionable attention と分離する。

### 非ゴール

- 本 phase で UI 全面刷新（配色・タイポ全面変更）はしない。
- 全 provider の 100% 一致保証は目標にしない（信頼度付き表示にする）。

## 4. UX 情報設計

## 4.1 画面構造

- 左: Sessions ツリー（常時）
- 上部: Status filter chips
  - `All / Attention / Running / Idle / Unmanaged`
- 中央: 選択 pane の内蔵 TTY

## 4.2 Pane 行の共通情報

- `status dot`（running/waiting/error/idle/unmanaged）
- `pane title`（rename > session label > pane title > fallback）
- `last active`（agent pane のみ）
- provider 表示（`co/cl/ge/...`）は補助情報として小さく表示（設定で ON/OFF）

## 4.3 Attention UI

- session ヘッダ右に `A<n>` バッジ（その session の actionable 件数）
- サイドバー上部に `Needs Attention` まとめ（任意で折りたたみ）
- `task_completed` は Attention ではなく Inbox へ（unread/ack 付き）

## 5. Attention 定義（新規）

`attention_state` を導入する。

- `none`
- `action_required_input`
- `action_required_approval`
- `action_required_error`
- `informational_completed`

分類ルール:

1. `waiting_input / waiting_approval / error` は actionable。
2. `running -> idle` だけでは attention を立てない。
3. `completed` は informational（Inbox）で保持。
4. actionable は解消イベントまで保持（フレーム揺れで消さない）。

## 6. Last Active 定義（新規）

`session_last_active_at` を導入し、pane 活動時刻と分離する。

### 6.1 表示ポリシー

1. `agent_presence=managed` のみ表示。
2. unmanaged/unknown は `-`（または非表示）。
3. 時刻は `session_last_active_at` を表示。
4. `session_time_confidence` が閾値未満なら空表示。

### 6.2 provider 別ソース優先度

#### Codex

1. app server thread `updatedAt`
2. runtime 最終 user input event
3. runtime 最終 non-admin event
4. unavailable -> unknown（updatedAt フォールバック禁止）

#### Claude

1. `--resume` で特定した session の `history.jsonl timestamp`
2. `projects/*.jsonl` の session mtime / preview
3. runtime 最終 user input event
4. unavailable -> unknown（pane updatedAt フォールバック禁止）

## 7. API 変更

`PaneItem` に以下追加:

- `attention_state`
- `attention_reason`
- `attention_since`
- `session_last_active_at`
- `session_time_source`
- `session_time_confidence`

`ListSummary` に以下追加:

- `by_attention_state`
- `actionable_attention_count`
- `informational_count`
- `session_time_known_rate`
- `session_time_match_rate`

## 8. 実装タスク分解

### Phase 20-A: Domain 追加

- `internal/stateengine/types.go` に attention / session-time 型を追加。
- canonical state とは独立に attention FSM を追加。

### Phase 20-B: Daemon 集約

- `buildPaneItems` で `attention_state` と `session_last_active_at` を計算。
- managed pane のみ時刻を返す。
- source/confidence を同時返却。

### Phase 20-C: Provider time resolver 強化

- codex: thread updatedAt 優先の時刻 resolver。
- claude: history/projects/resume を統合した時刻 resolver。
- 低信頼時は空返却。

### Phase 20-D: AppViewModel 切替

- by status view を filter lens 化（session tree を維持）。
- pane 行は `status dot + title + last active` の固定レイアウト。
- attention バッジ / Needs Attention セクション実装。

### Phase 20-E: Inbox 分離

- `task_completed` を informational queue へ移動。
- actionable と informational の表示・既読管理を分離。

### Phase 20-F: 観測性

- `attention_false_positive_rate`
- `attention_resolution_seconds`
- `session_time_known_rate`
- `session_time_match_rate`
- `session_time_unknown_rate`

## 9. テスト戦略

1. Unit
   - attention FSM（running->idle で attention にしない）
   - session-time resolver（source/confidence 付き）
2. Integration
   - `/v1/panes` と `/v1/snapshot` に新項目が一貫反映
   - managed/unmanaged で時刻表示ポリシーが守られる
3. UI tests
   - status filter が session 構造を崩さない
   - attention バッジ件数が actionable と一致
4. Replay
   - `claude idle but running` fixture
   - `codex waiting_input` fixture
   - `running->idle completed` fixture

## 10. 受け入れ条件（AC）

1. 主画面は by session 固定で運用できる（status は filter のみ）。
2. actionable attention は `waiting_input/approval/error` のみ。
3. `task_completed` は Inbox に入り、attention には入らない。
4. `last active` は managed pane のみ表示。
5. `session_last_active_at` が provider session 時刻由来で返る。
6. `session_time_known_rate >= 0.8`（managed pane）を達成。

## 11. リスクと対策

- リスク: provider 更新で時刻ソースが壊れる
  - 対策: source 別 fallback と replay fixture で検知
- リスク: attention の誤警報
  - 対策: actionable を最小集合に固定し、completion は informational 分離
- リスク: UI 情報過多
  - 対策: provider 表示は軽量化し設定で ON/OFF

## 12. 実装順（推奨）

1. 20-A/B を先行（API を先に固める）
2. 20-C で時刻精度改善
3. 20-D/E で UI 反映
4. 20-F で計測・チューニング

