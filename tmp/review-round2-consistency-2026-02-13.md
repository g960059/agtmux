# AGTMUX 整合性・網羅性レビュー（第2回）

**レビュー日**: 2026-02-13（修正反映後）
**レビュー観点**: ドキュメント間整合性・網羅性

---

## 前回指摘の対応状況

| 前回指摘 | 判定 | 備考 |
|---|---|---|
| 1-1: TC-046 ゲート配置 | **Resolved** | Phase 2 close に TC-046 追加済み |
| 1-4: adapter registry Phase 配置 | **Resolved** | TASK-035 Phase 2, TC-046 ゲート紐付き |
| 1-5: adapters テーブルタスク欠如 | **Partially Resolved** | TASK-001 が「all core tables」だが FR-11 未参照 |
| 2-1: FR-11 実装不在 | **Resolved** | TASK-035 が FR-11 担当 |
| 3-1: TC-044 タスク未参照 | **Resolved** | TASK-039 追加済み |
| 5-1: TC-018 フェーズ不整合 | **Resolved** | TASK-023 を Phase 1 に移動 |
| 6-2: daemon トランスポート未決定 | **Resolved** | spec 7.9 + plan Phase 0 に明記 |

---

## 新規指摘

### [Major] D-1: Plan Phase 0 スキーマリストの不完全性
implementation-plan Phase 0 in-scope のスキーマリストが `runtimes, events, event_inbox, runtime_source_cursors, states, actions, action_snapshots` のみ。spec 7.3 の `targets`, `panes`, `adapters` テーブルが欠落。特に `targets` と `panes` は TASK-031/032 で必須。

### [Major] D-2: TASK-001 の FR参照に FR-11 欠如
TASK-001 は「all core tables」のマイグレーションだが FR は `FR-3, FR-14, FR-15` のみ。adapters テーブルの責任タスクが曖昧。

### [Minor] D-3: TASK-039 ベンチマークの依存にアダプタ未含有
合成イベントのみでのベンチマークなら問題ないが、意図を明確化すべき。

### [Minor] D-4: TASK-040 と TASK-039 の TC-044 責任分界が不明確
両タスクが TC-044 を Test IDs に持つが、どちらが green 責任か不明。

### [Minor] D-5: TASK-023 と TC-053 のフェーズ分離意図が未記載
TASK-023 (Phase 1) が TC-053 を持つが、TC-053 は Phase 1.5 ゲート。分離意図が不明確。

### [Minor] D-6: spec Phase 2 exit criteria の簡潔さ
spec 側が1文のみで、plan の詳細シナリオ（disconnect/reconnect, event disorder）が反映されていない。

---

## 総合評価

| カテゴリ | 件数 |
|---|---|
| 前回指摘 Resolved | 6 |
| 前回指摘 Partially Resolved | 1 |
| 新規 Major | 2 (D-1, D-2) |
| 新規 Minor | 4 |

前回の7件中6件が解決。新規 Major は Plan のスキーマリスト補完で対処可能。
