# ADR 20260225: Core-first Execution Mode B

## Status
- Accepted

## Context
- 仕様レビューを反映する過程で、FR/Flow/Task が急増し、実装開始の摩擦が上がった。
- v5 の目的は「2層モデル + source分離 + pane signature v1」をまず動かすこと。
- docs-first の利点（compaction耐性）は維持しつつ、MVP実装速度を落とさない運用が必要。

## Decision
- 実行モードを `B (Core-first)` に固定する。
- `docs/20_spec.md` の FR を `[MVP]` と `[Post-MVP]` に分離する。
- Phase 1-2 の実装ブロッカーは `[MVP]` のみとする。
- `[Post-MVP]` は削除せず設計資産として保持し、必要時に昇格して実装する。
- `docs/40_design.md` は `Main (MVP Slice)` と `Appendix (Post-MVP Hardening)` に分離する。
- `docs/60_tasks.md` は `MVP Track` と `Post-MVP Backlog` を分離し、全タスクに `blocked_by` を持たせる。

## Consequences
- Positive:
  - MVP 実装に必要な判断が減り、着手しやすくなる。
  - hardening 仕様を失わず、必要時に段階的導入できる。
  - 依存関係が可視化され、タスク並行化しやすくなる。
- Negative / risks:
  - Post-MVP の放置で技術負債化する可能性がある。
  - MVP と hardening の境界判断が都度必要になる。

## Guardrails
- `[Post-MVP]` の前倒し実装は、実害が再現した場合に限定する。
- 前倒し時は `docs/60_tasks.md` と `docs/70_progress.md` へ理由と依存を記録する。
- `docs/10_foundation.md` の変更を伴う場合は必ずユーザーへエスカレーションする。

## Links
- `docs/00_router.md`
- `docs/20_spec.md`
- `docs/40_design.md`
- `docs/50_plan.md`
- `docs/60_tasks.md`
