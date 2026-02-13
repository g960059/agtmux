# AGTMUX v0.5 アーキテクチャ・技術設計 再レビュー報告

**レビュー日**: 2026-02-13
**対象バージョン**: v0.5 (3579e01)
**レビュー観点**: アーキテクチャ・技術設計（第2回）

---

## Part 1: 前回指摘の対応検証

### C-01: SSH接続管理 → **Resolved**
spec 7.1.2 に TargetExecutor and SSH Lifecycle セクションが新設。persistent connection reuse (ControlMaster-equivalent)、connect timeout: 3s、command timeout: 5s、retry policy (2 retries, exponential backoff 250ms/1s + jitter)、target health 遷移ルール (ok→degraded→down→ok) が全て定量化。

### C-02: 過剰設計 → **Partially Resolved**
Phase 0 exit criteria 定量化、タスク分解詳細化が進んだが、Phase 0 の 0a/0b 分割は未採用。Phase 0 は依然9タスクの大規模フェーズ。

### M-01: APIトランスポート → **Resolved**
spec 7.9 に normative 定義: HTTP/1.1 + JSON over UDS、socket path、permissions 0600、optional TCP loopback + token auth、GET /v1/health。

### M-03: State Engine/Reconciler責務境界 → **Resolved**
spec 7.1.1 State Ownership Boundary: states テーブルは State Engine のみ書き込み、Reconciler は合成イベント発行のみ。

### M-05: イベント伝搬二重パス → **Resolved**
7.1.1 により全ステート変更が State Engine 単一パス経由に。

### M-06: Watch バックプレッシャー → **Partially Resolved**
Watch JSONL Contract 定義済みだが、slow consumer 対処（バッファ上限、overflow ポリシー）が未定義。

### M-07: Reconciler スケーリング → **Partially Resolved**
基本動作定義済みだが、target 単位の scan budget、最大並列度が未規定。

### M-09/M-10: セキュリティ → **Resolved**
7.3.1 Data Protection 新設。connection_ref は non-secret のみ、raw_payload は redacted by default、retention policy 明記。

### M-12: event_inbox 複雑性 → **Resolved**
event_inbox テーブルと Runtime binding rule が詳細仕様化。pending_bind→bound→dropped_unbound ライフサイクル明確。

### M-13: adapter registry 配置 → **Resolved**
spec 7.1 に Adapter Registry 明記、adapters テーブル定義、adapter contract formalized。

### M-16: ゲート不一致 → **Resolved**
全フェーズの Phase Gate Bundles が具体的テストIDで定義。

### M-18: daemon lifecycle → **Resolved**
spec 7.9 に single-instance lock、graceful shutdown、restart cursor handling。

---

## Part 2: 新規指摘

### [Minor] N-01: Phase 0 タスク粒度
Phase 0 に9タスク集中。中間マイルストーン設定を推奨。

### [Major] N-02: Watch Stream バックプレッシャー未定義
daemon 側バッファ上限、overflow 時の動作（reset or 切断）が未規定。Phase 3 の macOS app が依存するため重要。

### [Minor] N-03: Reconciler 負荷制御の定量仕様不足
target 単位の同時スキャン上限、scan budget が未規定。

### [Major] N-04: TCP listener の token 認証方式が不十分
token 生成/配布/更新/失効/保存場所が未定義。Phase 3 前に仕様化が必要。

### [Minor] N-05: tmux_server_boot_id 安定性の前提条件
tmux 3.3 での boot_id 導出の具体的実装方式がタスクレベルで未担保。

### [Minor] N-06: action_snapshot の runtime_id 検証
snapshot 検証条件に runtime_id 一致チェックを明示すべき。

### [Minor] N-07: event_inbox の retention policy 未定義
bound 済み/dropped_unbound の purge タイミングが不明。

### [Info] N-08: Adapter health と target health の関係
adapter 固有の health が target health 遷移にフィードするかのルールが未明確。

### [Major] R-01: State Engine の CAS/serialization 方式未明示
concurrent state 更新時の排他制御がSQLite single-writer 依存か CAS かが不明。

### [Minor] R-02: unknown 状態の reason_code 正規リスト未定義

### [Major] X-01: Phase 1 の action 基盤責務範囲が plan に未明記
Phase 1 で attach 用 action 基盤を構築し Phase 1.5 で拡張する分担が plan 側で不明確。

### [Minor] X-02: TC-025/TC-026 が Phase 1 gate に未含有
TASK-018 (Phase 1) のテストが Phase 1.5 gate のみ。Phase 1 close 時の idempotency 検証が不足。

### [Minor] X-03: TASK-031 の acceptance criteria に異なる関心事が混在
TargetExecutor と daemon UDS transport が1タスクに混在。

---

## 総合判定: Go with changes

| カテゴリ | 件数 |
|---|---|
| 前回指摘 Resolved | 13 |
| 前回指摘 Partially Resolved | 3 |
| 新規 Major | 3 (N-02, N-04, R-01) |
| 新規 Minor | 7 |
| 新規 Info | 1 |

Phase 0 着手をブロックする理由はない。Phase 1 close 前に N-02 と R-01 の解決を推奨。
