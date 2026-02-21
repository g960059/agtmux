# Phase 22-26 実装設計（orchestration除外）

Date: 2026-02-20
Status: Draft v1 (implementation-ready)

Related:
- `docs/implementation-records/phase21-tty-v2-terminal-engine-detailed-design-2026-02-20.md`
- `docs/implementation-records/phase21-tty-v2-wire-schema-2026-02-20.md`
- `docs/implementation-records/phase21-d-v1-terminal-decommission-2026-02-20.md`
- `docs/implementation-records/phase21-e-ssh-hardening-2026-02-20.md`

## 0. 方針（今回の合意）

1. `orchestration` は当面スコープ外（Phase 22-26 には含めない）。
2. Phase 21 の中核判断を維持する。
   - daemon は VT を解釈しない。
   - daemon は raw bytes を透過中継する。
   - VT の正は SwiftTerm（client）に一本化。
3. 旧「daemon側差分計算（dirty row等）」は採用しない。
4. 優先順位は「正確な状態把握と注意喚起」>「過剰な描画最適化」。

---

## 1. 実行順（改訂）

- Phase 22: Terminal Data Plane 完成（raw bytes 経路の純化）
- Phase 23: Agent Adapter 基盤 v1
- Phase 24: Attention UX 再設計
- Phase 25: QoS + 描画パイプライン最適化
- Phase 26: Multi-target 運用強化

理由:
- カーソルずれ/表示崩れは「snapshot再構成依存」が主因で、まず Data Plane を完結させる必要がある。
- 状態精度（adapter）が低いまま UX 最適化しても通知品質が上がらない。
- QoS 最適化は adapter と attention の信頼できる入力が揃ってから効く。

---

## 2. 全体 Goal / Non-goal

## 2.1 Goal

1. 内蔵TTYの主経路で cursor/IME/scroll の再現性を安定化する。
2. `codex/claude/gemini` の状態推定を heuristic 主体から adapter 主体へ移行する。
3. attention を「要対応」に限定し、通知ノイズを下げる。
4. multi-target（特にSSH）で障害分離と遅延耐性を強化する。

## 2.2 Non-goal

1. orchestration（親子agent起動制御）の実装。
2. daemonでVT canonical grid を実装すること。
3. 全端末UIを chat app 化すること。

---

## 3. Phase 22: Terminal Data Plane 完成

## 3.1 目的

- selected pane の hot path を `capture-pane` 依存から切り離し、`tmux control mode -> tty-v2 -> SwiftTerm.feed(bytes)` を単一正規経路にする。

## 3.2 現状課題（解く対象）

1. selected pane でも snapshot再構成経路へ落ちる余地がある。
2. app 側で cursor 行/列推定ロジックが残っており、CJK/IMEでズレを誘発する。
3. `%output` の継続ストリームを使わず、capture由来の再描画が混在する。

## 3.3 設計

### 3.3.1 daemon

- 新規: `internal/daemon/tmux_control_bridge.go`
  - targetごとに control mode 常駐接続
  - `%output` を pane単位 bytes stream として publish
- `internal/daemon/tty_v2.go`
  - `output` frame は bridge経由 bytes のみ
  - `capture-pane` は下記限定:
    - attach直後の initial snapshot
    - sequence gap / reconnect後 resync
- 新規メトリクス:
  - `tty_hotpath_capture_count{selected=true|false,target=...}`
  - `tty_output_source{source=bridge|snapshot}`

### 3.3.2 macapp

- `macapp/Sources/AppViewModel.swift`
  - tty-v2 active時は `outputPreview` 文字列経路を read-only fallback に限定
  - selected pane の描画更新は bytes frame を直接 terminal view に渡す
- `macapp/Sources/NativeTmuxTerminalView.swift`
  - v2 active時は cursor位置推定/再配置ロジックを使わない
  - ESC[2J + 全文再描画相当パスを無効化
  - terminal内部カーソル（SwiftTerm）を正として表示

### 3.3.3 protocol

- 既存 `tty.v2.0` を維持
- 必要最小限の拡張のみ:
  - `output.source`（`bridge|snapshot`）を optional 追加
  - `attached.snapshot_mode`（`initial|resync`）を optional 追加

## 3.4 タスク分解

1. bridge 実装（per target）
2. bridge->tty-v2 output 配線
3. selected pane hot path の snapshot呼び出し禁止
4. appのcursor再計算ロジックを v2 path で無効化
5. resync条件の明文化（gap/reconnect/manual）
6. 計測フック追加
7. 回帰テスト追加

## 3.5 テスト

- Unit:
  - control mode `%output` parser
  - output seq monotonic / gap handling
- Integration:
  - attach -> write -> output（bridgeソース）
  - reconnect -> resync
- Replay:
  - codex prompt
  - claude tui + CJK
- AC:
  - selected pane 連続操作時 `tty_hotpath_capture_count(selected=true)==0`
  - cursorズレ再現シナリオ（既知ケース）で再発0

---

## 4. Phase 23: Agent Adapter 基盤 v1

## 4.1 目的

- provider/status 判定を heuristic 中心から adapter 中心へ移行。

## 4.2 設計

### 4.2.1 adapter contract

- 新規 package: `internal/agentadapter`
- interface 例:
  - `Detect(paneCtx) -> match/confidence`
  - `Snapshot(sessionRef) -> SessionSummary`
  - `Events(since) -> []AgentEvent`
- canonical event:
  - `running_started`
  - `waiting_input`
  - `waiting_approval`
  - `task_completed`
  - `error`

### 4.2.2 data pipeline

- 優先度:
  1. hook/wrapper event
  2. adapter snapshot
  3. heuristic fallback
- `internal/provideradapters` / `internal/stateengine` に統合
- confidence を保存し、UIで state source を持てるようにする

### 4.2.3 adapters

- Codex adapter v1
  - session metadata / local artifacts 読み取り
  - wrapper event 受信（将来hook対応前提）
- Claude adapter v1
  - hook/event を正規入力
  - `/resume` 相当タイトル/時刻へ寄せる
- Gemini adapter v1
  - 最低限 detect + idle/running 取得

## 4.3 タスク分解

1. adapter interface + registry
2. codex adapter v1
3. claude adapter v1
4. gemini adapter v1
5. stateengine接続
6. fallback heuristic縮退
7. 評価スクリプト（正解データ比較）

## 4.4 テスト/AC

- Unit: adapter parser/mapper
- Integration: event ingest -> pane state
- AC:
  - codex status precision >= 97%
  - claude status precision >= 95%
  - session title / last-active precision >= 90%

---

## 5. Phase 24: Attention UX 再設計

## 5.1 目的

- attention を「本当に対応が必要」な状態に限定し、ノイズを削減。

## 5.2 UX方針（tmux-first）

- 主表示は `By Session` 維持
- pane行に status dot + attention badge
- 上部attentionカウンタは「未確認件数（actionableのみ）」

## 5.3 attention taxonomy

- `none`
- `input_required`
- `approval_required`
- `error_required`
- `completed_notice`（短TTL, 任意表示）

## 5.4 ルール

1. `running -> idle` だけでは attention を立てない。
2. adapter event で actionable が来た場合のみ attention。
3. acknowledge（確認）で消える。TTL消去は notice のみ。

## 5.5 タスク分解

1. attention domain model 追加
2. queue model（unread/ack）整理
3. sidebar badge/filters 連動
4. notification policy（macOS通知）
5. settings 簡素化（attention関連のみ）

## 5.6 テスト/AC

- Unit: rule engine
- UI: filter/badge/ack
- AC:
  - actionable precision >= 90%
  - false-positive 30%以上削減

---

## 6. Phase 25: QoS + 描画パイプライン最適化

## 6.1 目的

- 入力体感とアニメーション滑らかさを安定化。

## 6.2 設計

### 6.2.1 scheduling

- selected pane: 高優先・低遅延
- managed/running (非選択): 中優先
- idle/unmanaged: 低優先（間引き）

### 6.2.2 client feed batching

- DisplayLink同期バッチング
- 8-12ms window で bytes coalesce
- frame budget超過時は「最新優先で中間破棄」

### 6.2.3 transport efficiency

- base64 overhead 計測を常時出力
- 閾値超過時のみ binary frame 実装を有効化
  - 条件例: CPU overhead > 15% or stream throughput > 1.5MB/s sustained

## 6.3 タスク分解

1. QoS class 定義
2. AppViewModel の stream優先度制御
3. NativeTmuxTerminalView feed バッチャ
4. telemetry拡張（p50/p95/p99 + dropped frames）
5. binary frame gate 実装（必要時）

## 6.4 テスト/AC

- Perf replay:
  - codex working stream
  - claude tui high-update
- AC:
  - local input p95 < 20ms
  - ssh input p95 < 45ms
  - median fps 55-60 安定

---

## 7. Phase 26: Multi-target 運用強化

## 7.1 目的

- target障害を隔離し、運用時の操作性を保つ。

## 7.2 設計

1. target別 circuit breaker
2. reconnect with jitter/backoff (per target)
3. target別 stream budget
4. health/liveness のUI可視化（簡潔）
5. failure domain 分離（ssh障害時もlocalは維持）

## 7.3 タスク分解

1. daemon target-qos manager
2. reconnect state machine 改修
3. app health indicators整理
4. timeout/retryポリシー target別化
5. 運用ガイド更新

## 7.4 テスト/AC

- Fault injection:
  - ssh timeout
  - intermittent disconnect
  - one-target high latency
- AC:
  - ssh障害時もlocal pane切替/入力劣化なし
  - 自動復帰成功率 >= 95%（通常ネットワーク条件）

---

## 8. 横断タスク

1. 互換整理
- `v1 terminal` は既定無効（Phase 21-D 済）
- 旧経路テストを段階的削除

2. observability
- `input_latency_ms`, `stream_latency_ms`, `fps`, `drop_count`, `capture_hotpath_count` を統一メトリクス化

3. quality gate
- 各phase完了時:
  - `go test ./...`
  - `cd macapp && swift test`
  - perf replay (scripted)
  - 2-codex review（1名はUI/UX観点）

---

## 9. 実装順序（Sprint単位）

Sprint 1: Phase 22-A/B/C
- bridge出力配線、selected hotpath capture禁止、cursor再計算経路停止

Sprint 2: Phase 22-D + Phase 23-A
- resync安定化 + adapter基盤投入

Sprint 3: Phase 23-B/C + Phase 24-A
- codex/claude adapter v1 + attention model

Sprint 4: Phase 24-B/C + Phase 25-A
- attention UI + QoS class

Sprint 5: Phase 25-B/C + Phase 26-A
- feed batching + multi-target隔離

Sprint 6: Phase 26-B/C + hardening
- reconnection/health最終調整 + docs更新

---

## 10. リスクと対策

1. control mode parser のイベント欠落
- 対策: parser fixture replay + property tests

2. adapter hook 依存の運用差
- 対策: fallback heuristic を残し confidenceで区別

3. QoSで stale view が増える
- 対策: selected pane 無条件優先 + manual resync

4. binary frame 導入時の互換問題
- 対策: capability negotiation で opt-in のみ

---

## 11. Definition of Done（全体）

1. Data Plane:
- selected pane で snapshot再構成が hot path から除去

2. 状態精度:
- codex/claude の state/title/last-active 精度が目標達成

3. Attention:
- actionable中心で運用可能（ノイズ許容範囲内）

4. 性能:
- local 55-60fps 安定、入力遅延目標達成

5. 運用:
- multi-target 障害分離が再現試験で確認済み

