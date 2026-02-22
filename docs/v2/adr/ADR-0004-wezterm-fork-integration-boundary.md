# ADR-0004: WezTerm Fork Integration Boundary

Date: 2026-02-21  
Status: Accepted  
Owner: AGTMUX Core

## Context

`ADR-0001` で fork 運用（更新ウィンドウ）は決めたが、実装時に最重要な点が未固定だった。

1. fork のどこを改造対象にするか
2. `wezterm-gui` の mux 前提と AGTMUX (`tmux-first`) をどう接続するか
3. どの範囲を「改造禁止」にして upstream drift を抑えるか

## Decision

fork戦略は次で固定する。

1. **fork一本 + renderer host 方針**
   `wezterm-gui` は terminal renderer / input host として使い、AGTMUX独自UX（sidebar, organize, context menu, DnD）は fork 側に追加する。
2. **mux差し替え禁止、bridge導入**
   wezterm mux を直接置換しない。fork 側 `wezterm-gui` に `AgtmuxRuntimeBridge` を追加し、protocol v3 の pane stream を `wezterm_term::Terminal` に feed する。
3. **tmux topology を唯一の正に固定**
   session/window/pane の構造正は daemon の `topology_sync/delta`。fork 側の pane tree は投影（projection）であり、正本にしない。
4. **fork surface を許可リストで管理**
   初期MVPで改造可能な領域は次に限定する。
   - `wezterm-gui/src/agtmux/*`（新規）
   - `wezterm-gui/src/gui/hooks/*` の最小フック
   - build / packaging / launcher の接続点
   それ以外（`termwiz`, `wezterm-term`, parser core, mux core）への変更は禁止。
5. **例外変更はADR必須**
   許可外ファイルを触る場合は ADR を先に起票し、理由と rollback を明記する。

## Consequences

メリット:

1. terminal correctness を upstream 実装で維持しやすい
2. sidebar / DnD 等の価値実装に集中できる
3. fork drift を変更面の制限で抑えられる

デメリット:

1. fork 内で UI hook 点を設計する初期コストがある
2. mux 連携の自由度は制限される

許容理由:

1. v2 の差別化は terminal 実装ではなく tmux operations UX のため
2. POCで再発した cursor/IME/scroll 問題を再導入しないため

## Follow-up

1. `specs/74-fork-surface-map.md` を正本にして改造面を管理
2. CI で許可外パス変更を fail するチェックを追加
3. Phase A/B に fork bridge skeleton を追加して着手順を固定

## Supersedes

`docs/v2/20-unified-design.md` と `docs/v2/30-detailed-design.md` の fork実装空白を解消する。
