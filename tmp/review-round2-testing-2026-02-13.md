# AGTMUX テスト・品質保証レビュー（第2回）

**レビュー日**: 2026-02-13（修正反映後）
**レビュー観点**: テスト・品質保証

---

## 前回指摘の対応状況

| 前回指摘 | 判定 |
|---|---|
| C-01: 境界値テスト欠落 | **Partially Resolved** — 基準値明確化されたが専用テストケース未追加 |
| C-02: tmux/SSH CI環境 | **Resolved** — TASK-040 + test-catalog Section 7 + plan Section 10 |
| C-03: テスト技術スタック | **Unresolved** — 実装言語・FW・CI ツール全て未定義 |
| M-01: Unit テスト不足 | **Partially Resolved** — TC-004, TC-007 のみ、追加なし |
| M-03: Pass Criteria 粒度 | **Partially Resolved** — 一部改善、約半数で曖昧な基準残存 |
| M-05: 異常系テスト不足 | **Partially Resolved** — TC-049〜053 追加、インフラ障害系は未カバー |
| M-06: Property テスト対象 | **Unresolved** — TC-006 のみのまま |
| M-07: Nightly 基盤 | **Resolved** — plan Section 10 + test-catalog Section 7 |
| M-10: 回帰テスト戦略 | **Partially Resolved** — ゲート累積で暗黙回帰、PR/リリースポリシー不足 |
| M-11: パフォーマンステスト | **Resolved** — TC-044 + TASK-039 + Benchmark Profile |
| M-13: Phase 1 実行順序 | **Resolved** — TASK-023 Phase 1 移動 |
| M-14: セキュリティテスト | **Partially Resolved** — TC-050 追加、UDS/TCP/injection 未カバー |

---

## 新規指摘

### [Critical] R-07: Adapter 間相互作用テストの欠落
複数 adapter が同時動作する環境でのテストが存在しない。Phase 2 で3 adapter が揃った際に顕在化。テスト設計は早期に開始すべき。

### [Major] R-02: TASK-040 の Acceptance Criteria が不十分
「green の定義」がない。CI パイプラインの設定ファイル等の成果物が Acceptance Criteria に含まれるべき。Nightly スケジュール（実行時刻、通知先）も未定義。

### [Major] R-04: テストピラミッドが逆三角形
Unit 2件 (3.8%) vs E2E+Integration 31件 (58.5%)。以下を Unit テストとして追加すべき:
1. State precedence 比較関数
2. `effective_event_time` 計算
3. `dedupe_key` 導出
4. `<ref>` BNF パーサー
5. `session-enc` percent-encoding/decoding
6. `pane_epoch` インクリメント判定
7. Target health state machine 遷移関数
8. Snapshot TTL 有効期限判定
9. Retention 期限計算
10. Adapter capability flag 解釈

### [Major] R-05: TASK-040/TASK-039 の TC-044 責任分界
両方が TC-044 を持つ。TASK-040 から TC-044 を除外し「Nightly で TC-044 実行可能にする」に変更すべき。

### [Major] R-08: ネガティブ入力テストカタログが不在
不正 JSON、不正 ref、不正 cursor、予期しない tmux 出力等のカタログが欠落。

### [Major] R-09: テスト並列安全性が不完全
SQLite DB、UDS socket、SSH port のテスト間分離戦略が未定義。

### [Minor] R-01: TC-044 に p99 閾値がない
p95 <= 2s のみ。p99 のウォーニング閾値を定義すべき。

### [Minor] R-03: CI 環境に macOS が含まれていない
Phase 3 の Manual+CI に macOS を明記すべき。

### [Minor] R-06: implementation-plan の Phase 2.5 gate bundle 記述の不正確さ
Plan が Phase 2 close のバンドルを Phase 2.5 として記載している箇所がある。

### [Minor] R-10: テスト命名規約の不在
TC-XXX の採番ルール、テスト関数名との対応規約が未定義。

### [Minor] R-11: Manual+CI テストの受け入れ基準が曖昧
手順書テンプレート、エビデンス最低要件、承認プロセスが不足。

---

## テストピラミッド現状

| 層 | テスト数 | 比率 |
|---|---|---|
| Unit | 2 | 3.8% |
| Property | 1 | 1.9% |
| Integration | 18 | 34.0% |
| Contract | 11 | 20.8% |
| E2E | 13 | 24.5% |
| Performance | 2 | 3.8% |
| Resilience | 3 | 5.7% |
| Security | 2 | 3.8% |
| Manual+CI | 1 | 1.9% |

## トレーサビリティ
FR-1〜16, NFR-1〜9 の全てが少なくとも1つのテストケースにマッピング済み。良好。

---

## 総合判定

| 重要度 | 件数 |
|---|---|
| Critical | 1 (R-07) |
| Major | 5 (R-02, R-04, R-05, R-08, R-09) |
| Minor | 5 |

**Phase 0 開始をブロックする Critical はなし**（R-07 は Phase 2 以降）。ただし R-04 (テストピラミッド) と C-03 (技術スタック) は Phase 0 早期に解決を強く推奨。
