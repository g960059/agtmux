# Contributing Guide (AGTMUX v2)

## Scope

このリポジトリは `docs/v2` を基準に v2 を再始動する。  
実装・設計変更は、まず `docs/v2` を更新してから行う。

## Required Read Order

実装前に必ず次を読む。

1. `docs/v2/00-index.md`
2. `docs/v2/10-product-charter.md`
3. `docs/v2/20-unified-design.md`
4. `docs/v2/30-detailed-design.md`
5. `docs/v2/40-execution-plan.md`

`docs/v2/references/*` は必要時のみ読む。

## Non-Negotiable Invariants

1. selected pane = stream-only
2. active path で snapshot/stream を混在させない
3. local echo しない
4. cursor/IME/scroll を app 側で推定しない
5. `wezterm-gui fork` 一本（thin integration は不採用）

## Document Editing Rules

1. 仕様の正本は `docs/v2/20-unified-design.md`
2. Data Model の正本は `docs/v2/30-detailed-design.md`
3. 実装順序・ゲートの正本は `docs/v2/40-execution-plan.md`
4. 教訓・再発防止は `docs/v2/50-poc-learnings.md`
5. UI検証運用は `docs/v2/60-ui-feedback-loop.md`
6. 過去資料は `docs/v2/references/` のみ

## Naming and Indexing

`docs/v2` 直下は番号付きファイルを維持する。

1. `00-` index
2. `10-` charter
3. `20-` unified design
4. `30-` detailed design
5. `40-` execution plan
6. `50-` learnings
7. `60-` operational guides

新規ファイルが必要な場合は `60-` 以降を使う。

## Change Control

変更は次の3分類で扱う。

1. Patch: 挙動不変の修正
2. Policy: UX/運用ルール変更
3. Constitutional: invariant 変更

Constitutional 変更は実装前に理由と影響を文書化する。

## Pull Request Checklist

1. 変更に応じて `docs/v2` を更新した
2. 影響範囲（state/protocol/ui/test）を明記した
3. acceptance criteria を明記した
4. rollback 手順を明記した
5. `docs/v2/00-index.md` との整合を確認した
6. UI変更時は `run-ui-feedback-report.sh` 結果を残した

## Context Budget Rule (for implementation agents)

実装中は、必読5ファイル以外をデフォルトで開かない。  
追加ファイルを読む場合は、必要理由を1行で記録する。
