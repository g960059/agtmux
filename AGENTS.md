# AGENTS.md

このリポジトリは AGTMUX v2 の再始動用です。実装・設計の判断は `docs/v2` を正本にします。

## 1. 最初に読む順番（必須）

1. `docs/v2/00-index.md`
2. `docs/v2/10-product-charter.md`
3. `docs/v2/20-unified-design.md`
4. `docs/v2/30-detailed-design.md`
5. `docs/v2/40-execution-plan.md`

`docs/v2/references/*` は必要時のみ参照。

## 2. 絶対に守る不変条件

1. `selected pane = stream-only`
2. active path で snapshot と stream を混在させない
3. local echo をしない
4. cursor/IME/scroll を app 側で推定しない
5. data plane と control plane を分離する
6. `wezterm-gui fork` 一本（thin integration は採用しない）
7. fork改造範囲は `docs/v2/specs/74-fork-surface-map.md` に従う

## 3. 変更管理ルール

変更は次の3分類で扱う。

1. `Patch`: 挙動不変の修正
2. `Policy`: UX/運用ルール変更
3. `Constitutional`: 不変条件に触る変更

`Policy` 以上の変更では、実装前に docs を更新し、影響範囲と受け入れ条件を明記する。

## 4. ドキュメント正本

1. 仕様方針: `docs/v2/20-unified-design.md`
2. Data Model / protocol: `docs/v2/30-detailed-design.md`
3. 実装順序 / gate: `docs/v2/40-execution-plan.md`
4. 教訓 / 再発防止: `docs/v2/50-poc-learnings.md`
5. wire/gate/bootstrap: `docs/v2/specs/*`
6. 意思決定: `docs/v2/adr/*`
7. fork境界: `docs/v2/specs/74-fork-surface-map.md`

過去資料は `docs/v2/references/*` に隔離し、通常作業では読まない。

## 5. 実装時の最小コンテキスト方針

1. まず必読5ファイルのみ読む
2. 追加資料を読む場合は「読む理由」を1行で残す
3. 無関係ファイルの探索を避ける

## 6. PR/変更前チェック

1. 不変条件を破っていない
2. 変更内容に応じた docs 更新を行った
3. 受け入れ条件（AC）を定義した
4. rollback 手順を定義した
5. `docs/v2/00-index.md` の導線を壊していない

## 7. ファイル配置ルール

1. 新規設計文書は `docs/v2` 配下に追加する
2. 番号付き命名を維持する（`60-` 以降を使用）
3. 実験メモや比較資料は `docs/v2/references` に置く
