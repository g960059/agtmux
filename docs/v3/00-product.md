# Product Overview

## 解決する問題

AI coding agent (Claude Code, Codex, Gemini CLI, Copilot) を tmux で複数同時に動かすと:

1. **どの agent が何を待っているか分からない** — approval 待ちの pane に気づかず放置してしまう
2. **手動で各 pane を巡回する必要がある** — 5+ pane になると確認だけで集中が途切れる
3. **agent の状態が不透明** — running/idle/error の区別がつかない

これらは agent 1台なら問題にならないが、**複数 agent の並行運用** では生産性の大きなボトルネックになる。

## Product Vision

**tmux 上の AI agent の状態を正確に推定し、ユーザーの注意を必要な pane に誘導する。**

ユーザーは agent の状態を一切気にせず自分の作業に集中できる。
attention が必要なときだけ通知され、適切な pane に即座に移動できる。

## Persona

### Primary: マルチ Agent パワーユーザー

- tmux を常用し、複数の AI agent を同時に動かす開発者
- Claude Code + Codex を併用、将来的に Gemini/Copilot も追加
- 1 セッションに 3〜10 個の agent pane を持つ
- agent の approval/input 待ちに気づかず数分放置した経験がある
- terminal 操作に慣れており、CLI ツールに抵抗がない

### Secondary: tmux-status ユーザー

- tmux status line に情報を集約する習慣がある
- 常時視界に入る場所で agent 状態を確認したい
- 設定は `.tmux.conf` に1行追加するだけにしたい

## User Stories

### Core (MVP)

1. **状態確認**: 「全 agent pane の状態を一覧で見たい」
   - `agtmux status` で全 session/pane の状態をワンショット表示
   - running / waiting_input / waiting_approval / idle / error が区別できる

2. **Attention 通知**: 「approval 待ちや input 待ちの pane にすぐ気づきたい」
   - attention 状態の pane がハイライトされる
   - state 変化から 3 秒以内に表示に反映

3. **即座の移動**: 「attention が必要な pane にワンアクションで移動したい」
   - TUI で Enter を押すと `tmux select-pane` で即移動

4. **常時モニタリング**: 「作業中も agent 状態を視界の隅で把握したい」
   - `agtmux tui` でライブ更新
   - `agtmux tmux-status` で tmux status line に常時表示

5. **正確な状態推定**: 「表示される状態が実際の agent 状態と一致していてほしい」
   - false positive（実際は idle なのに running と表示）が少ない
   - approval 待ちを見逃さない（high recall）

### Extended (Phase 4+)

6. **精度の自己検証**: 「推定精度を数値で把握したい」
   - `agtmux accuracy` で precision/recall/F1 レポート

7. **Desktop 表示**: 「terminal 以外の UI でも同じ情報を見たい」
   - Tauri desktop app で sidebar + xterm.js terminal (Phase 5)

## Product Principles

1. **正確性 > 機能数** — 状態推定が不正確なら、機能がいくら多くても価値はゼロ
2. **CLI-first** — terminal ユーザーが最速で価値を得られる形で提供する
3. **非侵入的** — agent の動作を一切変えない。観察のみ
4. **漸進的** — CLI → TUI → tmux-status → Desktop と段階的に体験を拡張

## Success Metrics

| Metric | Target | 測定方法 |
|--------|--------|---------|
| approval 待ち検知率 (recall) | >= 90% | live validation |
| 状態表示の false positive rate | <= 10% | live validation |
| attention 表示遅延 | < 3s | p95 latency |
| Daily Active Usage | 開発日の 80%+ | opt-in telemetry (将来) |
