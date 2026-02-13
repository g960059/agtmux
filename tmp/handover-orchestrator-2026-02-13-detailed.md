# AGTMUX Orchestrator Handover (Detailed)

- Generated at: 2026-02-13T01:02:25-0800
- Repository: `/Users/virtualmachine/ghq/github.com/g960059/agtmux`
- Branch: `main`
- Scope: Spec整備後の実装準備（plan/tasks/testカタログ整備 + Codex 4並列レビュー反映）

## 1. Executive Summary

このラウンドで実施したことは以下です。

1. `docs/agtmux-spec.md` を前提に、実装直前に必要な運用ドキュメントを起票。
2. `docs/implementation-plan.md`, `docs/tasks.md`, `docs/test-catalog.md` を新規作成。
3. Codex 4並列レビューを実行し、主要指摘を3ドキュメントへ反映。
4. FR/NFRトレースの穴埋めを再確認し、最終整合を実施。

現時点で、仕様から実装タスクへの落とし込みは「フェーズ・依存・テストゲート」まで一貫しています。

## 2. 生成・更新アーティファクト

### 2.1 Docs

- `docs/agtmux-spec.md`（既存更新・大幅差分）
- `docs/implementation-plan.md`（新規）
- `docs/tasks.md`（新規）
- `docs/test-catalog.md`（新規）

参考: `wc -l`
- `docs/agtmux-spec.md`: 701
- `docs/implementation-plan.md`: 174
- `docs/tasks.md`: 71
- `docs/test-catalog.md`: 120

### 2.2 Review artifacts（Codex 4並列）

今回の反映判断の主根拠:
- `tmp/agtmux_codex_review_artifacts_plan.md`
- `tmp/agtmux_codex_review_artifacts_tasks.md`
- `tmp/agtmux_codex_review_artifacts_tests.md`
- `tmp/agtmux_codex_review_artifacts_trace.md`

補助ファイル（前段レビュー）:
- `tmp/agtmux_codex_review_p0p1_arch.md`
- `tmp/agtmux_codex_review_p0p1_plan.md`
- `tmp/agtmux_codex_review_p0p1_reliability.md`
- `tmp/agtmux_codex_review_p0p1_ux.md`
- その他 `tmp/agtmux_codex_review_*_v2.md`

## 3. 反映した重要判断（レビュー対応）

### 3.1 `attach` と idempotency の順序逆転を修正（Critical）

問題:
- fail-closed action 要件上、`attach` も shared action write/idempotency 基盤に乗るべきだが、タスク順が逆になりうる構成だった。

反映:
- `TASK-018` を Phase 1 側に前倒し。
- `TASK-015` が `TASK-018` に依存するよう修正。

該当:
- `docs/tasks.md` (`TASK-015`, `TASK-018`)
- `docs/implementation-plan.md` (Phase 1 in-scope)

### 3.2 Phase gate の必須束を強化（Critical/High）

問題:
- セキュリティ/性能/watch互換などがゲートで弱く、フェーズ通過判定が甘くなるリスク。

反映:
- Phase 0 gate に `TC-011, TC-012, TC-013` と基盤テストを追加。
- Phase 1 gate に latency (`TC-044`), watch schema (`TC-045`), grouping/multi-target (`TC-042/043`), continuity (`TC-051`) を明示。
- Plan 側に「bundle all greenでなければclose不可」を明文化。

該当:
- `docs/test-catalog.md` (Section 3)
- `docs/implementation-plan.md` (Gate Binding, Exit criteria)

### 3.3 Phase 0の必須コンポーネントを明記（High）

問題:
- `TargetExecutor` / tmux observer / daemon boundary が Phase 0成果物として曖昧。

反映:
- Phase 0 in-scopeに明記。
- 専用タスク `TASK-031`, `TASK-032` を追加。
- 対応テスト `TC-040`, `TC-041` を追加。

該当:
- `docs/implementation-plan.md`
- `docs/tasks.md`
- `docs/test-catalog.md`

### 3.4 FR/NFRトレース穴の解消（High）

問題:
- FR-7/FR-9/FR-11, NFR-5/NFR-6 等の見え方が弱い箇所があった。

反映:
- Grouping/aggregation: `TASK-012`, `TASK-033`, `TASK-034`, `TC-017`, `TC-042`, `TC-043`
- Adapter registry/version: `TASK-035`, `TASK-036`, `TC-046`, `TC-047`
- Dedupe robustness: `TASK-004`, `TASK-038`, `TC-007`, `TC-049`

カバレッジ確認結果（スクリプト実行済み）:
- `FR present: [1..16]`
- `NFR present: [1..9]`
- `Missing FR: []`
- `Missing NFR: []`

### 3.5 DoD と運用モードの矛盾修正（High）

問題:
- CIのみをDone条件にすると、Nightly/Manual+CIのテスト運用と矛盾。

反映:
- DoDを `CI + Nightly + Manual+CI証跡` に整合化。

該当:
- `docs/tasks.md` Section 3

## 4. 仕様（spec）側で既に押さえられている契約（実装前提）

実装時に破らない前提として重要な点:

- Runtime guard: `runtime_id` / `pane_epoch` と stale rejection
- Action fail-closed: server-side `action_snapshot` + precondition validation
- Action idempotency: `request_ref` と `E_IDEMPOTENCY_CONFLICT`
- Watch resume: `cursor=<stream_id:sequence>`
- Watch JSONL schema stability (`watch --format jsonl`)
- Default grouping: `target-session`
- Ordering: `source_seq`/`effective_event_time`/`ingested_at`/`event_id`
- Clock skew: `skew_budget`（既定10s）で `event_time` vs `ingested_at` フォールバック

参照（検索済みキーワード）:
- `request_ref`, `stream_id:sequence`, `pane_epoch`, `E_IDEMPOTENCY_CONFLICT`, `target-session`, `watch --format jsonl`, `action_snapshot`, `skew_budget`

## 5. 現在のワークツリー状態

`git status --short` 抜粋:
- Modified: `docs/agtmux-spec.md`
- Untracked: 
  - `docs/implementation-plan.md`
  - `docs/tasks.md`
  - `docs/test-catalog.md`
  - `tmp/agtmux_codex_review_artifacts_*.md`
  - `tmp/agtmux_codex_review_p0p1_*.md`
  - `tmp/handover-2026-02-13.md`

注意:
- まだ commit はしていない。
- 実装コード変更・テスト実行は未実施（ドキュメント整備のみ）。

## 6. Orchestrator向け推奨アクション（次ラウンド）

### 6.1 直近の進行順（依存を崩さない最小経路）

1. Phase 0 kickoff（Core runtime）
- `TASK-001` → `TASK-002/003/004/005/006`
- 並行で `TASK-031`, `TASK-032`
- ゲート: `TC-001..013 + TC-040/041/049/050`

2. Visibility MVP（Phase 1）
- `TASK-009..014`
- `TASK-018`（idempotency基盤）→ `TASK-015`（attach）
- `TASK-016/017/033/034`
- ゲート: Phase 1 bundle

3. Control MVP（Phase 1.5）
- `TASK-019/020/021` + `TASK-022/023`
- ゲート: `TC-025..032 + TC-053`

4. Reliability + Adapter expansion（Phase 2/2.5）
- `TASK-024..026` → `TASK-035/036/037` → `TASK-027/028`

### 6.2 並列実行の推奨レーン

- Lane A（DB/State Engine）: `TASK-001..008, TASK-038`
- Lane B（Target/Topology）: `TASK-031, TASK-032, TASK-009`
- Lane C（API/Watch/CLI）: `TASK-012..014, TASK-016, TASK-017, TASK-033, TASK-034`
- Lane D（Action Path）: `TASK-018, TASK-015, TASK-019..023`
- Lane E（Adapter Extensibility）: `TASK-024..028, TASK-035..037`

## 7. 未解決リスク・注意点

- `docs/agtmux-spec.md` の更新量が大きい（701行）ため、実装前に「contract freeze対象節の再確認」を実施推奨。
- `TC-038/039` は Manual+CI。Phase 3に向けてrunbookテンプレートを早めに用意した方がよい。
- `Nightly` 系 (`TC-012/013/034/052`) はCI設計が未着手なので、早期にworkflow草案化が必要。

## 8. 受け渡しチェックリスト（Orchestrator用）

- [ ] 4文書（spec/plan/tasks/test）の同期更新方針をチームに共有
- [ ] まず Phase 0 の owner とマイルストーンを確定
- [ ] テストIDベースでPRテンプレートを更新（Task ID + Test ID + Gate Impact）
- [ ] CI/Nightly/Manual+CI の実行責務を明確化
- [ ] ドキュメント一式をcommit単位に分割するか（spec+docs / artifacts分離）を決定

## 9. 参考コマンド（再確認用）

```bash
# 変更状況
git status --short

# FR/NFRカバレッジ再確認
python3 - <<'PY'
import re, pathlib
files=['docs/tasks.md','docs/test-catalog.md','docs/implementation-plan.md']
text='\n'.join(pathlib.Path(f).read_text() for f in files)
fr=sorted({int(m.group(1)) for m in re.finditer(r'FR-(\d+)', text)})
nfr=sorted({int(m.group(1)) for m in re.finditer(r'NFR-(\d+)', text)})
print('FR present:', fr)
print('NFR present:', nfr)
print('Missing FR:', [i for i in range(1,17) if i not in fr])
print('Missing NFR:', [i for i in range(1,10) if i not in nfr])
PY

# レビュー成果物確認
ls -1 tmp/agtmux_codex_review_artifacts_*.md
```

---

このhandoverは「実装を開始できる状態に整える」目的で作成しています。次担当は、上記の依存順を崩さずに Phase 0 実装へ着手してください。
