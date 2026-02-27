# Lessons Learned (Self-Improvement Log)

> ユーザーから修正を受けたら、ここにパターンを記録する。
> セッション開始時に関連 lessons があれば確認する。
> 形式: `## YYYY-MM-DD — <タイトル>` + 原因・教訓・防止策。

---

## 2026-02-27 — Subagent delegation の遵守率が低い

**状況**: T-121〜T-126 の実装全般で、Orchestrator がコード実装・テスト実行・レビューを直接行い、
subagent への委任をほぼしなかった（遵守率 ~35%）。

**根本原因**:
- 委任ルールが `docs/00_router.md` にのみ記載 → auto-load されない → 実質的に守られない
- "少しだけ直接修正する" という小さな逸脱が積み重なる

**教訓**: 守られるべきルールは CLAUDE.md（常時ロード）に書く。router.md のみは不十分。

**防止策**: CLAUDE.md Hard Gate #1 に明記済み。

---

## 2026-02-27 — Plan 承認後に docs 更新をスキップして実装に直行

**状況**: Plan mode で `.claude/plans/` にプランを作成 → ユーザー承認 → そのまま実装開始。
`docs/60_tasks.md` / `docs/70_progress.md` が更新されないまま実装が進む。

**根本原因**:
- Plan mode のアウトプット（`.claude/plans/`）が "公式" に見える
- docs 更新は意識的な摩擦ステップであり、実装モメンタムに負ける

**教訓**: `.claude/plans/` は scratch。承認後の最初のアクションは docs 更新と plan ファイル削除。

**防止策**: CLAUDE.md Hard Gate #2 に明記済み。

---

## 2026-02-27 — Multi-phase タスクの Phase 2/3 が docs に反映されない

**状況**: T-126 が 3 phase 構成だったが、コンテキスト圧縮後に Phase 2/3 の詳細が
`docs/70_progress.md` に残っておらず、次セッションでの再確認が困難だった。

**根本原因**: "タスク完了時にまとめて書く" という defer 習慣。
コンテキスト圧縮がフェーズ間で発生すると情報が失われる。

**教訓**: 各フェーズ完了直後に書く。defer は情報損失と等価。

**防止策**: CLAUDE.md Hard Gate #3 に明記済み。

---

## 2026-02-27 — Review Pack なし・reviewer なし・GO 判定なしでコミット

**状況**: T-121〜T-126 全タスクで Review Pack が作成されず、Codex reviewer も呼ばれず、
そのまま commit / push が行われた。

**根本原因**: Review ルールが router.md の深い場所にのみ記載。
`just verify` PASS = 完了 という誤認識。

**教訓**: verify は最低限の gate。review は別の独立した gate。

**防止策**: CLAUDE.md Hard Gate #4 に明記済み。

---

## 2026-02-27 — 実装レベルの fallback 多用による根本原因の隠蔽

**状況**: JSONL path encoding、pane detection、CWD 解決などで
「失敗したら別の方法で推測」するパターンが複数重なり、
どこで何が失敗しているかが見えなくなった（T-122, T-126 等）。

**根本原因**: "なんとか動く" を目標にした防衛的実装。
監視ツールでは silent wrong answer が silent no answer より危険。

**教訓**: 実装レベルの fallback は silent failure を作る。エラーを surface せよ。
アーキテクチャ設計上の tier 降格のみ許可。

**防止策**: CLAUDE.md Code Quality Policy に明記済み。
