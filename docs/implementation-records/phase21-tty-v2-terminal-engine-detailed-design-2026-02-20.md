# Phase 21: TTY v2 Terminal Engine 詳細設計 (2026-02-20, Revised after review)

Status: Draft (implementation-ready, review-aligned)
Related:
- `docs/implementation-records/phase21-tty-v2-wire-schema-2026-02-20.md`
- review input: `/tmp/phase21-tty-v2-design-review.md`
- review input: `/tmp/phase21-wire-schema-and-design-review.md`

## 0. 要約

本設計は、AGTMUX 内蔵TTYを `tmux-first` の思想を維持したまま、native terminal レベルの応答性・安定性へ引き上げるための **非互換前提の再設計**である。

レビューを踏まえた最終方針:

1. tmux連携は `capture-pane` ポーリングを廃止し、**control mode 常駐接続**へ移行する。
2. app-daemon間は `/v1/terminal/*` の都度取得を廃止し、**`/v2/tty/session` 単一双方向ストリーム**へ統合する。
3. daemon canonical grid / delta-ops 方式は採用しない。daemonは **raw bytes transparent proxy + flow control** を担う。
4. VT解釈は1箇所（SwiftTerm）に集約し、エミュレータ層の重複を避ける。
5. 入力・リサイズ・出力・状態イベントを同一ストリームで順序保証し、**backpressure + 最新優先 drop** で詰まりを回避する。
6. topology/classifier は前景TTYパスから分離し、UI反応性を優先する。

---

## 1. 背景と現行ボトルネック

現行実装の詰まりどころ（コード根拠）:

- クライアント短周期ポーリング: `macapp/Sources/AppViewModel.swift:421`, `macapp/Sources/AppViewModel.swift:2780`
- 入力直後の追加取得が走る構造: `macapp/Sources/AppViewModel.swift:987`
- サーバが要求ごとに capture-pane 系実行: `internal/daemon/server.go:1841`, `internal/daemon/server.go:6142`
- `exec.CommandContext` による都度プロセス起動: `internal/target/executor.go:49`
- UDS接続の都度 open/close: `macapp/Sources/CommandRuntime.swift`
- 描画が実質フルクリア前提: `macapp/Sources/NativeTmuxTerminalView.swift:972`

これにより、入力遅延のばらつき、CPUスパイク、フリッカー、pane増加時のスケール劣化が発生する。

---

## 2. Goal / Non-goal

### 2.1 Goal

1. 入力反映 `p95 < 16ms`（local target）
2. pane切替反映 `p95 < 80ms`
3. 描画 `55-60fps` 維持（通常稼働）
4. tmux 子プロセス churn をホットパスから排除
5. pane数増加時の劣化を段階的（線形近似）に抑える

### 2.2 Non-goal

1. tmux を廃止すること
2. CLIごとの独自chat UIを作ること
3. 本phaseで全OS向け同時最適化（まず mac app + local/ssh target）
4. daemon 内で VT エミュレータを自前実装すること

---

## 3. レビュー反映と意思決定

## 3.1 反映した指摘

1. **daemon canonical grid は撤回**（Critical）
2. Section 6.4 の delta ops 方式は撤回し、`output(raw bytes)` フレームへ置換
3. SSH hardening は独立 phase として切り出し
4. 複数client resize policy を設計で明文化

## 3.2 継続採用

1. control mode bridge
2. `/v2/tty/session` 双方向ストリーム
3. backpressure / watermark / coalescing
4. foreground/background scheduler 分離
5. observability + performance budget gate

---

## 4. 全体アーキテクチャ

```
tmux (target local/ssh)
   │
   │ control mode (persistent)
   ▼
TmuxControlBridge (per target)
   │ %output (raw bytes), layout, lifecycle events
   ▼
TTY Stream Hub (/v2/tty/session, persistent UDS stream)
   │ multiplexed frames (output/state/ack/error)
   ▼
mac app TTYTransportSession
   │
   ▼
SwiftTerm.feed(rawBytes)  // VT parse is single source of truth
```

補助経路（低優先）:

- Topology/Snapshot/Classifier worker（sidebar更新用）
- Metrics/Tracing exporter

補足:

- 非選択paneの切替時は cold path で `capture-pane` を許容（resync用途のみ）。
- hot path での `capture-pane` は禁止。

---

## 5. Daemon 設計

## 5.1 新規コンポーネント

1. `TmuxControlBridge`
   - targetごとに1インスタンス
   - tmux control mode の stdin/stdout を常駐管理
   - `%output`, `%layout-change`, `%session-changed`, `%window-add`, `%window-close`, `%exit` を解析

2. `TTYSessionRouter`
   - app接続ごとに stream session を管理
   - attach/write/resize/focus/detach のルーティング

3. `PaneStreamBuffer`
   - paneごとに短い byte ring buffer を保持（例: 64-256KB, tunable）
   - 非選択paneはバッファ保持のみ、push頻度を抑制

4. `ForegroundScheduler`
   - selected pane のI/Oイベントを最優先処理

5. `BackgroundScheduler`
   - classifier/topology/summary を低優先で処理

## 5.2 状態モデル（daemon）

`PaneStreamState`:

- `pane_id`
- `target`
- `session_name`
- `window_id`
- `cols`, `rows`（直近tmux情報）
- `output_seq` (u64)
- `input_ack_seq` (u64)
- `ring_buffer`
- `last_output_at`
- `last_input_at`
- `activity_state`
- `attention_state`

注記:

- grid/cell の正規状態は daemon で保持しない。
- 描画状態の正はクライアント（SwiftTerm）側。

## 5.3 並行性

- グローバル `terminalMu` は廃止。
- target/pane単位のactor + channelで並行処理。
- 共有辞書のみ細粒度 lock。

## 5.4 backpressure

- 接続ごとに `egress queue` を保持。
- watermark:
  - `soft`: coalescing開始（同pane outputを結合）
  - `hard`: 中間 output フレーム破棄（最新優先）
- 重要フレーム（error/detach/ack/state）は破棄禁止。

---

## 6. Wire Protocol (`/v2/tty/session`)

## 6.1 Transport

- UDS persistent connection（1接続長寿命）
- フレームは length-prefixed JSON（初期）
- `protocol_version` + `capabilities` で将来のbinary化に備える

## 6.2 Client -> Daemon frames

1. `hello`
   - `client_id`, `protocol_version`, `capabilities`
2. `attach`
   - `request_id`, `target`, `session_name`, `window_id`, `pane_id`
3. `write`
   - `request_id`, `pane_ref`, `input_seq`, `bytes_base64`
4. `resize`
   - `request_id`, `pane_ref`, `cols`, `rows`, `resize_seq`
5. `focus`
   - `pane_ref`
6. `detach`
   - `pane_ref`
7. `ping`
   - `ts`
8. `resync`
   - `pane_ref`, `reason`

## 6.3 Daemon -> Client frames

1. `hello_ack`
   - `server_id`, `protocol_version`, `features`
2. `attached`
   - `pane_ref`, `initial_snapshot_ansi_base64`, `state`
3. `output`
   - `pane_alias|pane_ref`, `output_seq`, `bytes_base64`
4. `state`
   - `pane_ref`, `activity_state`, `attention_state`, `session_last_active_at`
5. `ack`
   - `request_id`, `input_seq` or `resize_seq`
6. `error`
   - `request_id?`, `code`, `message`, `recoverable`
7. `detached`
   - `pane_ref`, `reason`
8. `pong`
   - `ts`

注記:

- raw bytes proxy モデルでは daemon 側で VT カーソル位置を正確に再構成しない。
- カーソル描画は SwiftTerm の VT 解釈結果を唯一の正とする。

## 6.4 Resync policy

1. attach直後: `initial_snapshot_ansi` + 以後 output stream
2. unexpected sequence gap（coalesced=false）検知時: daemon が `capture-pane` 1回で再同期
3. resync中は output を一時バッファし、snapshot後に追従分を送出

---

## 7. Client (macapp) 設計

## 7.1 新規/刷新コンポーネント

1. `TTYTransportSession`
   - UDS persistent stream 管理
   - reconnect/backoff
   - in/out frame queue

2. `TTYOutputApplier`
   - `attached.initial_snapshot_ansi` を `SwiftTerm.feed()`
   - `output.bytes_base64` を `SwiftTerm.feed()`
   - sequence gap時は `resync` 発行

3. `RenderScheduler`
   - DisplayLink同期（max 60fps）
   - 高頻度 output 時の feed バッチング

4. `InputPipeline`
   - key/IME/paste を `write` frame化
   - `input_seq` 発番

## 7.2 廃止対象

1. `outputPreview` を正とする経路
2. `terminalStreamLoop` ポーリング前提
3. 即時追い読み (`performViewOutput`) ホットパス
4. full repaint 依存レンダリング

---

## 8. tmux control mode 統合

## 8.1 起動

- target接続確立時に `tmux -CC` セッションを生成/再利用。
- bridgeは target lifecycle と同一。

## 8.2 イベント処理

- `%output`: 対象 pane stream へ raw bytes enqueue
- `%layout-change`: pane size 更新
- `%session-changed` 等: topology worker へ通知
- `%exit`: target unhealthy として reconnect

## 8.3 コマンド送信

- `write` は control mode 経由で対象paneに注入。
- `resize` は複数client policyに従って実行。

---

## 9. 複数クライアントとresize policy

1. `focus` を最後に送った client を authoritative とする。
2. authoritative client 以外の resize 要求は `skipped_conflict` ack を返す。
3. authoritative client 切断時は次の focus client へ移譲。
4. tmux `aggressive-resize` 依存はドキュメント化し、テストfixtureで固定確認。

---

## 10. エラー回復

1. stream切断
   - exponential backoff で再接続
   - reconnect後 `hello -> attach -> resync`

2. target down
   - pane state を `degraded` 表示
   - target復旧で自動再attach

3. tmux bridge crash
   - bridge再起動
   - 既存clientは `error(recoverable=true)` 受信後、resync

4. resync失敗
   - fallbackとして external terminal open 導線を提示

---

## 11. 観測性 (必須)

## 11.1 Metrics

- `tty_input_latency_ms` (p50/p95/p99)
- `tty_render_fps`
- `tty_stream_rtt_ms`
- `tty_output_bytes_per_sec`
- `tty_output_base64_overhead_ratio`
- `tty_output_drop_count`
- `tty_resync_count`
- `tmux_bridge_reconnect_count`
- `tty_queue_depth_current/max`

## 11.2 Tracing

- keypress -> write enqueue -> daemon ack -> first visible update
- pane switch -> first paint

## 11.3 Budget Gate

fail条件:

1. input latency p95 >= 20ms（local profile）
2. pane switch p95 >= 120ms
3. render fps p50 < 50

---

## 12. セキュリティ/認証

1. UDS peer credential（macOS: `getpeereid`）で接続元プロセスを識別。
2. app bundle ID / uid を許可リストで検証（開発時は緩和可）。
3. `session_token` は必須化しない（必要時のみオプション）
4. stale runtime guard は v2 でも維持。

---

## 13. 移行計画（非互換）

## 13.1 Phase 21-A: Protocol Skeleton + Control Mode Bridge

- `/v2/tty/session` の hello/attach/write/output 最小実装
- local target の control mode 常駐接続
- `%output` -> raw bytes -> client の最短経路確立
- この時点で性能測定（base64 overhead を含む）

## 13.2 Phase 21-B: macapp Stream Client

- `TTYTransportSession` 実装
- SwiftTerm raw bytes feed 直結
- `outputPreview` 主経路撤去
- pane切替時の snapshot resync

## 13.3 Phase 21-C: Scheduler + Backpressure

- foreground/background 分離
- queue watermark + coalescing + latest-wins drop
- 非選択pane更新の間引き

## 13.4 Phase 21-D: Remove v1 Terminal APIs

- `/v1/terminal/*` 実使用停止・削除
- capture-pane hot path の禁止を CI で検証

## 13.5 Phase 21-E: SSH Target Hardening (separate)

- SSH target で control mode 運用
- reconnect/latency/jitter チューニング
- multiplexing 条件の運用ガイド化

---

## 14. テスト戦略

## 14.1 Unit

1. stream frame parser/serializer
2. input_seq/ack ordering
3. backpressure drop policy
4. focus-authoritative resize policy

## 14.2 Integration

1. `attach -> write -> output -> ack` 正常系
2. reconnect/resync
3. bridge crash recovery
4. multi-pane/multi-target 負荷

## 14.3 Replay

1. codex 長文ストリーム
2. claude TUI 更新頻発
3. pane切替連打
4. resize連打 + output高負荷

## 14.4 Soak

- 8時間連続稼働
- FDリーク、メモリ増加、queue飽和有無

---

## 15. 受け入れ条件 (AC)

1. 入力反映 `p95 < 16ms`（local）
2. pane切替反映 `p95 < 80ms`
3. render `>=55fps`（通常運用）
4. capture-pane が hot path で 0
5. `/v1/terminal/*` が運用経路に残らない
6. selected pane の取りこぼし・混線が再現しない
7. go/swift 主要回帰テストが green

---

## 16. リスクと対策

1. control mode parser の複雑化
   - 対策: parser層を独立、fixture replay tests

2. JSON frame の高頻度負荷
   - 対策: metrics監視、閾値超過時 binary framing を phase 22 で導入

3. SSH遅延の揺らぎ
   - 対策: SSH hardening phase を独立運用

4. 複数clientの表示差異
   - 対策: authoritative focus policy + external terminal fallback

---

## 17. Open Questions

1. JSON frame のまま性能目標を満たせるか（必要なら binary frame）
2. base64 encode の帯域/CPU overhead が許容範囲か（Phase 21-Aで計測）
3. pane alias 圧縮を v2.0 で入れるか v2.1 に送るか
4. pane ring buffer の適正サイズ（memory vs catch-up）
5. tmux control mode event 欠落時の resync閾値
6. SSH target の推奨構成（multiplexing / keepalive / reconnect）

---

## 18. 実装開始条件

1. `/v2/tty/session` frame schema 凍結（v2.0 draft）
2. control mode fixture 一式
3. performance budget テスト雛形
4. rollback/kill-switch 手順（開発期間のみ）
