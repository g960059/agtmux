# AGTMUX Product Charter (v1.0)

Date: 2026-02-21  
Status: Active  
Purpose: 開発中の仕様・実装・UI判断をぶらさないための最上位指針

## 1. この文書の役割

この文書は「何を守り、何を変えてよいか」を固定する。
実装の都合、短期の不具合、見た目の好みで、根本方針を変更しないための憲章である。

優先順位:

1. `この憲章`
2. `docs/v2/20-unified-design.md`
3. `docs/v2/30-detailed-design.md`
4. `個別PR判断`

## 2. 背景と存在理由

AGTMUX は、tmux を使った複数エージェント運用で発生する以下の運用コストを下げるために作る。

1. pane を順に巡回しないと状態がわからない
2. waiting_input / approval 待ちを見逃す
3. target（local/VM/SSH）を跨いだ状況把握が遅い
4. tmux階層（session/window/pane）の情報はあるが、運用上は agent状態が見たい

## 3. North Star（不変）

**tmux first のまま、agent運用を最短で判断・介入できること。**

言い換え:

1. 実行基盤は tmux
2. 観測・操作基盤は AGTMUX
3. 主役は window ではなく pane（agent runtime）

## 4. 対象ユーザーとユースケース

## 4.1 Primary User

1. tmux で日常的に開発する本格ユーザー
2. 複数agentを並列で走らせるユーザー

## 4.2 Secondary User

1. tmux知識が浅いが、app上でagent運用したいユーザー

## 4.3 Core User Stories

1. どの pane が `running / waiting / idle / error` かを即時把握したい
2. 注意が必要な pane だけに集中したい
3. pane を選んだらそのまま操作し、入力したい
4. local/ssh を跨いでも同じ操作感で管理したい

## 5. 絶対に守る不変条件（Architecture Invariants）

以下は L0 不変。破る場合は「再始動レベルの意思決定」が必要。

1. `selected pane = stream only`
2. snapshot と stream を active 描画で混在させない
3. local echo を使わず、表示は stream 結果のみ
4. cursor/IME/scroll は terminal engine を真実源とし、推定しない
5. control plane と data plane を分離する
6. target 障害時も全体表示は partial result で継続する
7. fork戦略は `renderer host + agtmux UI layer` 境界を維持し、terminal core/mux core を安易に改造しない

## 6. UX 不変条件（Product Invariants）

1. 操作の主語は `pane`。session/window は補助メタ情報。
2. 並び順は安定（stable）を基本にし、自動で頻繁に飛ばない。
3. 重要通知は attention queue に集約し、ノイズ通知を抑制する。
4. main は terminal 操作導線を優先し、補助フォーム依存に戻さない。

## 7. 状態モデル方針（State Policy）

## 7.1 Canonical activity

1. `running`
2. `waiting_input`
3. `waiting_approval`
4. `idle`
5. `error`
6. `unknown`

## 7.2 Attention policy

attention は「ユーザー介入が必要な時だけ」発火する。

1. `task_complete`
2. `waiting_input`
3. `waiting_approval`
4. `error`

`running -> idle` のみで全件通知しない。

## 7.3 判定ソース優先順位

1. hooks
2. wrapper events
3. adapterが読む会話履歴/メタ
4. output heuristic（最後のfallback）

## 8. 変更可能領域と凍結領域

## 8.1 いつでも変更してよい（L2）

1. 配色、余白、ラベル文言
2. メニュー配置
3. 低リスクのUX磨き込み

## 8.2 影響評価つきで変更（L1）

1. sort/filterルール
2. sidebar情報密度
3. attention表示方式

変更時は「ユーザーストーリーへの影響」を明記する。

## 8.3 原則固定（L0）

前述の Architecture Invariants。

## 9. 意思決定ルール（ぶれ防止）

機能追加・方針変更前に、必ず次の質問に答える。

1. これは North Star を改善するか？
2. 不変条件（L0）に触れていないか？
3. ユーザーの「判断速度」を上げるか？
4. 運用ノイズ（認知負荷）を増やしていないか？

1つでも No があるなら実装しない。先に設計を見直す。

## 10. Spec Change Protocol（仕様変更手順）

仕様が揺れないよう、変更を3種類に分類する。

1. `Patch`（軽微）: 挙動不変の修正
2. `Policy`（中）: UX/判定ルール変更
3. `Constitutional`（重）: L0不変の変更

必須手順:

1. 変更提案を docs に記録
2. 影響範囲（state/protocol/ui/test）を明記
3. acceptance criteria を先に定義
4. 実装後、回帰テストとレビュー結果を記録

Constitutional 変更は、別ドキュメントで「なぜ壊してよいか」を先に承認する。

## 11. 実装フェーズ運用ルール

各phase開始時に以下を固定する。

1. Goal
2. Non-goal
3. 触るファイル範囲
4. Gate（何が通れば次へ進むか）

各phase終了時に以下を残す。

1. 変えたこと
2. 変えなかったこと
3. 既知の残課題
4. 次phaseへの前提

## 12. DoD（Definition of Done）

実装完了と呼べる条件:

1. 不変条件を破っていない
2. テストが通っている（unit/integration）
3. 体験上の退行がない（入力/スクロール/表示）
4. docs が更新されている
5. ロールバック可能な単位でコミットされている

## 13. 失敗シグナル（早期停止条件）

次の兆候が出たら、実装追加ではなく設計見直しを優先する。

1. 同種バグ（cursor/scroll/jitter）が3回以上再発
2. hot path に暫定分岐が増え続ける
3. 仕様説明より例外説明の方が長くなる
4. phase内で invariant 違反を容認し始める

## 14. 最後の原則

AGTMUX は「tmuxを置き換えるUI」ではない。  
**tmux運用の意思決定速度を上げる運用OS** である。

この原則に反する変更は、便利そうでも採用しない。
