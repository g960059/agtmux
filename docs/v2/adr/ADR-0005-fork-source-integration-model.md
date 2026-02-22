# ADR-0005: Fork Source Integration Model

Date: 2026-02-21  
Status: Accepted  
Owner: AGTMUX Core

## Context

fork戦略として `wezterm-gui fork` 一本は確定しているが、次が未固定だった。

1. AGTMUX 本体repoへ fork ソースをどう取り込むか
2. desktop host を独立実装とするか、forkベース実装とするか
3. CI で fork drift と改造範囲をどう管理するか

## Decision

次の統合モデルを採用する。

1. **two-repo model**
   - AGTMUX core repo: daemon/protocol/store/docs
   - WezTerm fork repo: desktop UI host (`wezterm-gui` + AGTMUX layer)
2. **submodule pin**
   - AGTMUX core repo では `third_party/wezterm` を submodule として pin する
   - 通常開発で submodule pointer は変更しない
3. **desktop 実体**
   - desktop host は独立 terminal app を新規実装しない
   - desktop build/run は fork 側 `wezterm-gui` の AGTMUX mode を使う
4. **更新運用**
   - pointer 更新は ADR-0001 の update window（隔週）でのみ許可
   - update window 以外の pointer 変更は fail

## Consequences

メリット:

1. core repo と fork repo の責務が明確
2. fork巨大差分が core repo 履歴を汚しにくい
3. update window で再現性の高い追随運用ができる

デメリット:

1. submodule 運用の習熟が必要
2. CI が2段（core/fork）になる

許容理由:

1. v2 初期は実装速度より drift 制御と安定性を優先するため

## Follow-up

1. `specs/72-bootstrap-workspace.md` に submodule 取り込み手順を追加
2. `specs/74-fork-surface-map.md` に core/fork の CI 分担を明記
3. `specs/75-fork-hook-map-spike.md` で hook 点特定を最初に実施
