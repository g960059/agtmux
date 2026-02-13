実行可能性レビュー結果です。`docs/tasks.md` 単体では実行時に詰まる箇所が複数あります。

**指摘（重大度順）**
1. `Critical` 要件トレーサビリティに欠落があります。`FR-7/FR-9/FR-11` と `NFR-4/NFR-5/NFR-6` がタスク・テストに未割当です（要件定義: `docs/agtmux-spec.md:73`, `docs/agtmux-spec.md:75`, `docs/agtmux-spec.md:77`, `docs/agtmux-spec.md:89`, `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:91`、タスク表: `docs/tasks.md:10`、テスト表: `docs/test-catalog.md:22`）。このままだと「完了判定不能」になります。  
改善案: 欠落要件を満たす専用タスクを追加し、対応 `TC` を新設してください。

2. `Critical` `attach` と `idempotency` の依存順が逆です。`TASK-015` が `TASK-018` より先に成立する定義ですが（`docs/tasks.md:26`, `docs/tasks.md:29`）、仕様は「全アクションで snapshot + idempotency 必須」です（`docs/agtmux-spec.md:506`, `docs/agtmux-spec.md:510`、`docs/implementation-plan.md:16`, `docs/implementation-plan.md:17`）。  
改善案: `TASK-018` を Phase 1 に前倒しし、`TASK-015` の依存に `TASK-018` を追加。

3. `Critical` DoD とテスト運用モードが矛盾しています。DoD は CI 通過必須（`docs/tasks.md:58`）ですが、カタログは `Nightly`/`Manual+CI` を含みます（`docs/test-catalog.md:35`, `docs/test-catalog.md:36`, `docs/test-catalog.md:57`, `docs/test-catalog.md:61`, `docs/test-catalog.md:62`）。  
改善案: DoD を `CI必須 + Nightly最新成功 + Manual証跡必須` に分解。

4. `High` `TC-018` の割当がフェーズと不整合です。`TASK-012` に `TC-018` が紐付いています（`docs/tasks.md:23`）が、`TC-018` は action 系エラーも対象です（`docs/test-catalog.md:41`）。一方、write API は Phase 1.5 扱いです（`docs/implementation-plan.md:91`）。  
改善案: `TC-018` を read/action で分割するか、`TASK-023` 側へ再割当。

5. `High` Immediate Sprint が依存閉包になっていません。候補に `TASK-012` があるのに依存 `TASK-006` が候補外です（候補: `docs/tasks.md:50`、依存定義: `docs/tasks.md:23`）。  
改善案: 候補に `TASK-006` を追加するか、`TASK-012/013` を次スプリントへ移動。

6. `Medium` タスク粒度と受入条件が粗く、検証基準が曖昧です。例: `TASK-001`, `TASK-009`, `TASK-012`, `TASK-019` は範囲が広すぎ、`stable/convergent/works` 表現は測定不能です（`docs/tasks.md:12`, `docs/tasks.md:20`, `docs/tasks.md:23`, `docs/tasks.md:30`, `docs/tasks.md:35`, `docs/tasks.md:36`）。  
改善案: 1タスク1契約に分割し、受入条件を `Given/When/Then + 数値閾値` 化。

**修正案（そのまま使える差分例）**
```diff
--- a/docs/tasks.md
+++ b/docs/tasks.md
@@
-| TASK-012 | 1 | P0 | Implement API v1 read endpoints | FR-4, FR-6 | TASK-004, TASK-006 | panes/windows/sessions JSON contract stable | TC-017, TC-018 | Todo |
+| TASK-012 | 1 | P0 | Implement API v1 read endpoints | FR-4, FR-6 | TASK-004, TASK-006 | panes/windows/sessions JSON contract stable | TC-017 | Todo |
@@
-| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
+| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012, TASK-018 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
@@
-| TASK-018 | 1.5 | P0 | Implement `actions` write path with idempotency | FR-15 | TASK-015 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
+| TASK-018 | 1 | P0 | Implement `actions` write path with idempotency | FR-15, NFR-4 | TASK-003, TASK-012 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
+| TASK-031 | 1 | P0 | Implement grouping/summary counters (`target-session` default) | FR-7, FR-9 | TASK-012 | session/window rollup exposes state counts and default grouping correctness | TC-040 | Todo |
+| TASK-032 | 1 | P0 | Implement adapter registry + contract version gate | FR-11, NFR-5, NFR-6 | TASK-001 | adding a new adapter requires no core-engine change and version mismatch is rejected | TC-041, TC-042 | Todo |
```

```diff
--- a/docs/tasks.md
+++ b/docs/tasks.md
@@
-- Linked tests are automated and passing in CI.
+- Linked CI tests are automated and passing in CI.
+- Linked Nightly tests are green in the latest scheduled run.
+- Manual+CI tests include reproducible evidence in PR.
```

**受入条件テンプレート例（検証可能化）**
```md
Given: 3 targets / 60 active panes / 10 events-per-sec profile
When: run `watch` for 10 minutes under flap scenario
Then: p95 visible lag <= 2s, no duplicate cursor sequence, `partial=true` includes failed target details
Evidence: CI artifact `perf/watch-latency.json`
```

前提: ドキュメントレビューとして実施（コード未実装のため実行確認は未実施）。
