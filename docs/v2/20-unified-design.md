# AGTMUX v2 Unified Design

Date: 2026-02-21  
Status: Draft (execution baseline)  
Owner: AGTMUX Core

## 1. Purpose

この文書は AGTMUX v2 の唯一の統一設計書である。  
`tmux-first` を維持しつつ、pane 運用の速さと window 編集の柔軟性を両立する。

この文書は次を統合する。

1. Product charter の不変条件
2. Phase28 の運用設計
3. Rust rewrite 案の技術的強み

重要方針:

1. UI host strategy は `wezterm-gui fork` 一本で固定する
2. `thin integration` は v2 では採用しない

## 2. North Star

**tmux を実行基盤として維持し、複数 agent pane の状況把握と介入を最短化する。**

v2 の価値は terminal 実装そのものではなく、運用判断速度にある。

## 3. Hard Invariants

1. selected pane は stream-only で表示する
2. active 表示で snapshot と stream を混在させない
3. local echo は行わない
4. cursor/IME/scroll を app 側で推定しない
5. data plane と control plane を分離する
6. target 障害時も partial result を継続する
7. sidebar の並びは stable を基本にする
8. tmux window を第一級操作対象として扱う

## 4. Scope

1. local + ssh target
2. pane-centric default UI
3. optional window-grouped view
4. in-app terminal operation
5. adapter-driven state engine (codex/claude/gemini)
6. in-app DnD layout mutation (tmux command backed)

## 5. Non-goals

1. tmux を置き換える独自 multiplexer 開発
2. v2 初期での orchestration DAG
3. 現行 PoC wire 互換
4. custom GPU terminal renderer の初期導入
5. WezTerm thin integration 路線

## 6. Operating Model

1. 日常運用の主語は pane
2. 構造編集の主語は window
3. 境界管理の主語は session
4. 1 session = 1 repo を強制しない

## 7. UX Information Architecture

## 7.1 Sidebar

1. Header: `Sessions`, `create new pane`, `organize`
2. Filter tabs: `all | managed | attention | pinned`
3. Session blocks: accordion, stable order, DnD reorder
4. Pane rows: `title + state dot + relative time`
5. Footer: settings

## 7.2 View modes

1. `Pane-centric By Session` (default)
2. `Window-grouped` (optional)
3. `Chronological` (optional organize mode)

## 7.3 Context menus

Pane item:

1. Open Pane
2. Open tmux Window
3. Rename Pane
4. Kill Pane
5. Pin/Unpin

Session item:

1. Create New Pane
2. Rename Session
3. Pin/Unpin Session
4. Kill Session

## 7.4 Window as first-class operation

1. window card 表示を提供する
2. window context open を提供する
3. window 単位 state summary を表示する
4. window 間 pane 移動を DnD で実行可能にする

## 8. Domain Model

1. Target
2. TmuxSession
3. TmuxWindow
4. TmuxPane
5. AgentRuntime
6. AttentionItem
7. LayoutMutation

Ref identity:

1. TargetRef `{target_id}`
2. SessionRef `{target_id, session_id}`
3. WindowRef `{target_id, session_id, window_id}`
4. PaneRef `{target_id, session_id, window_id, pane_id, pane_epoch}`

## 9. Architecture

## 9.0 UI host strategy (fixed)

v2 は UI host を次で固定する。

1. `agtmux-desktop` は `wezterm-gui fork` を基盤にする
2. terminal correctness は fork 側 terminal stack をそのまま利用する
3. sidebar/menu/dnd は fork 側 UI に拡張実装する
4. thin integration は採用しない

理由:

1. cursor/IME/scroll/CJK を Day 1 で安定させるため
2. custom terminal renderer の再発リスクを避けるため
3. tmux operations UX への開発リソース集中のため

## 9.0.1 Fork implementation model (fixed)

fork 実装モデルを次で固定する。

1. `wezterm-gui` を **renderer host** として使う
2. AGTMUX 専用機能は `agtmux UI layer` として fork 内に追加する
3. `wezterm mux` は置換しない（直接改造しない）
4. tmux topology は daemon の `topology_sync/delta` を唯一の正とする
5. desktop は `AgtmuxRuntimeBridge` で protocol v3 と terminal feed を接続する

この境界の詳細は `./adr/ADR-0004-wezterm-fork-integration-boundary.md` と `./specs/74-fork-surface-map.md` を正本とする。

## 9.1 Runtime components

1. `agtmuxd-rs` daemon
2. `agtmux-desktop` (`wezterm-gui` fork)
3. `agtmux-cli`
4. `agtmux-agent-adapters`
5. `agtmux-store` (SQLite)
6. `agtmux-protocol` (binary frame)

## 9.2 Data plane

1. pane output raw bytes stream
2. selected pane high-priority lane
3. local primary source: `pipe-pane -O`
4. ssh primary source: remote tap bridge

## 9.3 Control plane

1. topology sync
2. attach/focus/write/resize
3. create/kill/rename/pin
4. layout mutate API
5. state/attention events

## 9.4 Adopted technical profile

1. Language: Rust
2. Terminal state: `wezterm-term` (commit pinned)
3. Terminal helpers: `termwiz`
4. Async: `tokio`
5. Storage: `rusqlite` + WAL
6. UI host: `wezterm-gui fork` (single strategy)

## 9.5 Explicit non-adoption (v2 initial)

1. `iced + custom wgpu terminal renderer`
2. daemon/client の terminal 二重状態管理
3. snapshotベース active redraw
4. `WezTerm thin integration`（forkなし混成構成）
5. `wezterm mux` 全面差し替え

## 9.6 Fork surface governance

1. 通常変更は `specs/74-fork-surface-map.md` の allowed zones に限定する
2. restricted zones の変更は ADR 承認前に行わない
3. CI で restricted zone 変更を検知し、ADR 未添付なら fail する
4. fork update window（ADR-0001）ごとに replay gate を必須実行する

## 9.7 Fork source integration model

1. core repo と fork repo を分離する（two-repo model）
2. core repo は `third_party/wezterm` submodule pin で fork を参照する
3. desktop host は fork 側 `wezterm-gui` の AGTMUX mode を使う
4. `apps/desktop-launcher` は起動・配布補助に限定する

詳細は `./adr/ADR-0005-fork-source-integration-model.md` を正本とする。

## 10. Protocol v3

Transport:

1. UDS local transport
2. length-prefixed binary frames
3. per-pane monotonic sequence

Core frames:

1. hello / hello_ack
2. topology_sync / topology_delta
3. attach / attached
4. focus / write / resize / ack
5. output
6. state
7. layout_mutate / layout_preview / layout_commit / layout_revert
8. error / metrics

Contract:

1. `output.source` in `pane_tap | bridge_fallback | preview`
2. `pane_epoch` mismatch は fail-closed
3. `mutation_id` idempotent
4. `request_id` ack 必須
5. window-capable 互換のため `focus_level=window|pane` を許容

## 11. Stream and Mutation FSM

Pane stream FSM:

1. detached
2. recovering
3. live

Rules:

1. recovering -> live は first output 受信でのみ遷移
2. live で snapshot-like frame を受理しない
3. stream break 時は recovering へ戻し再attach

Layout mutation FSM:

1. idle
2. previewing
3. pending_commit
4. committed
5. reverted

Rules:

1. per-window lock
2. timeout は revert
3. topology snapshot で最終整合

## 12. State and Attention

Presence:

1. managed
2. unmanaged
3. unknown

Activity:

1. running
2. waiting_input
3. waiting_approval
4. idle
5. error
6. unknown

Attention:

1. task_complete
2. waiting_input
3. approval
4. error
5. none

Source priority:

1. hooks
2. wrapper events
3. adapter metadata (`/resume` 相当)
4. heuristic fallback

Title priority:

1. user rename
2. provider session title
3. provider transcript excerpt
4. tmux metadata fallback

Last-active policy:

1. managed pane は provider session last time 優先
2. unmanaged pane は last-active 非表示

## 13. tmux Layout Mutation Mapping

1. swap-pane
2. move-pane
3. join-pane
4. break-pane
5. resize-pane
6. select-layout

DnD drop targets:

1. left
2. right
3. top
4. bottom
5. tab
6. new-window

## 14. Workspace Layout

```text
agtmux/
  Cargo.toml
  third_party/
    wezterm/
  crates/
    agtmux-protocol/
    agtmux-target/
    agtmux-tmux/
    agtmux-state/
    agtmux-agent-adapters/
    agtmux-store/
    agtmux-daemon/
    agtmux-cli/
  apps/
    desktop-launcher/
  scripts/
    ui-feedback/
  docs/
    architecture/
    implementation-records/
    runbooks/
```

## 15. Performance SLO

1. local input p95 < 20ms
2. local active fps median >= 55
3. ssh input p95 < 80ms
4. pane switch p95 < 120ms
5. layout mutate commit p95 < 250ms
6. selected stream gap p95 < 40ms

実装段階での暫定閾値は `./specs/71-quality-gates.md` の stage gate を使う。

## 16. Metrics and Gates

Required metrics:

1. fps p50/p95
2. input latency p50/p95
3. stream latency p50/p95
4. selected hotpath capture count
5. stream source ratio
6. layout mutation success rate
7. state source ratio
8. window focus switch latency

Merge gates:

1. selected pane capture count == 0
2. cursor/scroll/IME replay pass
3. window focus attach E2E pass
4. layout rollback E2E pass
5. SLO budget within threshold

## 17. Delivery Plan (Phases)

1. P28-A: workspace bootstrap + protocol codec
2. P28-B: stream core + pane tap manager
3. P28-C: terminal integration + IME verification
4. P28-D: sidebar IA v2 + organize/filter
5. P28-E: window-capable UX
6. P28-F: DnD layout mutation
7. P28-G: adapter-first state/attention
8. P28-H: ssh hardening + release gates

## 18. Risk Controls

1. WezTerm drift -> commit pin + periodic update window
2. layout race -> per-window lock + mutation_id
3. adapter drift -> fixture contract tests
4. ssh jitter -> target isolation + retry/backoff
5. update regression -> replay suite + fast rollback
6. fork scope creep -> fork surface map + owner rule + ADR gate

Accepted ADRs:

1. `./adr/ADR-0001-wezterm-fork-branch-strategy.md`
2. `./adr/ADR-0002-ssh-tunnel-framing.md`
3. `./adr/ADR-0003-notification-scope.md`
4. `./adr/ADR-0004-wezterm-fork-integration-boundary.md`
5. `./adr/ADR-0005-fork-source-integration-model.md`

## 19. Decision Policy

仕様変更時は次を必須とする。

1. Goal / Non-goal
2. Impacted invariants
3. Acceptance criteria
4. Rollback plan

L0 不変条件に触る場合は ADR を先に作成し、承認前に実装しない。

## 20. Final Position

v2 は「terminal app」ではなく「tmux operations OS」として作る。  
terminal correctness は WezTerm stack で担保し、差別化は次に置く。

1. pane/window 操作速度
2. state/attention 精度
3. multi-target 運用耐性
