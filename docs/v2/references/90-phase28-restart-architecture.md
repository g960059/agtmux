# Phase 28: AGTMUX v2 Reboot Master Design (tmux-first, pane-first + window-capable)

Date: 2026-02-21  
Status: Draft v2 (rewrite baseline)  
Scope: 新規プロジェクトとして再始動するための最上位実装設計

## 0. Decision Summary

この Phase 28 は、既存 PoC を継続改修する方針を終了し、`tmux-first` を保ったまま AGTMUX を Rust + WezTerm 系アーキテクチャで再起動するための基準設計である。

最重要決定:

1. 実行基盤は tmux を維持する。tmux を置き換えない。
2. 操作の主語は pane（agent runtime）を基本とする。
3. ただし tmux window は第一級操作対象として扱う（閲覧・open・layout編集）。
4. selected pane は stream-only（snapshot 混在禁止）。
5. terminal correctness（cursor/IME/scroll）は WezTerm terminal stack を真実源とする。
6. state/attention は adapter first（hooks/wrapper/event）で判定する。

## 1. 背景と再起動理由

PoC で価値は検証できたが、次が構造的ボトルネックだった。

1. snapshot と stream の混在により VT 状態が破綻しやすい。
2. cursor 推定や UI 補正が増殖し、再発バグが止まらない。
3. input/render/control の境界が曖昧で遅延やガタつきが出る。
4. session/window/pane の操作体系がUIと実装で分断されている。

再起動の目的は、パッチ蓄積ではなく「再発不能な構造」に置き換えること。

## 2. Product North Star と不変条件

`../10-product-charter.md` を上位とし、Phase 28 は以下を実装不変とする。

1. selected pane = stream-only
2. active path で snapshot/stream を混在させない
3. local echo しない（表示は tmux 側反映結果のみ）
4. cursor/IME/scroll を app 側で推定しない
5. control plane / data plane を分離
6. target 障害でも partial result で継続
7. 並び順は stable を基本にし、操作中に勝手に飛ばない
8. tmux window を操作対象として提供する（閲覧だけで終わらせない）

## 3. Scope / Non-goal

## 3.1 Scope (v2 初期リリース)

1. local + ssh target
2. pane-centric UI（default）+ window-grouped UI（optional）
3. in-app terminal（WezTerm engine）
4. pane/window 操作（open, create, rename, kill, layout mutate）
5. adapter-driven state/attention（codex/claude/gemini）

## 3.2 Non-goal

1. tmux 非依存の独自 multiplexer を作ること
2. orchestration（親子agent DAG）を v2 初期で実装すること
3. 現行 wire/API 互換
4. 既存 Swift 実装の部分移植

## 4. Domain Model (Canonical)

## 4.1 Core Entities

1. `Target`
2. `TmuxSession`
3. `TmuxWindow`
4. `TmuxPane`
5. `AgentRuntime`
6. `AttentionItem`
7. `LayoutMutation`

## 4.2 Identity Rules

1. `TargetRef = {target_id}`
2. `SessionRef = {target_id, session_id}`
3. `WindowRef = {target_id, session_id, window_id}`
4. `PaneRef = {target_id, session_id, window_id, pane_id, pane_epoch}`
5. `AgentRef = {provider, runtime_id}`

`pane_epoch` は stale action 防止用に必須。

## 4.3 Session / Window / Pane の位置づけ

1. pane は日常運用の最小操作単位（attach/input/kill/state表示）
2. window は構造編集単位（grouping/layout mutate/open window context）
3. session は運用境界単位（project に限らない）

## 5. UX Information Architecture

## 5.1 基本方針

1. default は `Pane-centric By Session`
2. optional に `Window-grouped` を持つ
3. `By status` は主導線から外し、filter で解決する

## 5.2 Sidebar 構造

1. Header: `Sessions`, `create new pane`, `organize`
2. Filter tabs: `all | managed | attention | pinned`
3. Body: session block の stable list
4. Footer: settings

## 5.3 Session Block 仕様

1. Session title row
2. Hover時のみ `create new pane` アイコン表示
3. 左アイコンは `folder`、in-flight 時は `spinner`
4. Accordion 開閉を提供（アニメーション必須）
5. 右クリックメニュー: `Rename Session`, `Pin/Unpin`, `Kill Session`

## 5.4 Pane Row 仕様

1. 行は1段表示を基本（title + relative time + state dot）
2. title 優先順位:
   - user renamed pane
   - adapter session title (resume由来)
   - last user input excerpt
   - fallback (`pane_title` or `current_cmd`)
3. relative time は managed pane のみ表示
4. 右クリックメニュー:
   - `Open Pane`
   - `Open tmux Window`
   - `Rename Pane`
   - `Kill Pane`
   - `Pin/Unpin`

## 5.5 Window 表示仕様

`Window-grouped` モードで次を必須化する。

1. session 内に window card を表示
2. 各 card は `window name/index + state summary`
3. pane は card 内に所属表示
4. `Open tmux Window` は window コンテキストに main terminal を切り替える
5. `Show tmux windows` は organize で切り替える（settings には置かない）

## 5.6 Organize Menu

1. `Organize by: Session | Window-grouped | Chronological`
2. `Sort by: Stable(manual) | Updated | Created`
3. `Show window grouping: On | Off`
4. メニューは右クリックメニューと同じ密度（小サイズ、単層）

## 6. tmux Window / Pane Operation Design

## 6.1 Open semantics

1. `Open Pane`: `select-window + select-pane` 後に pane stream attach
2. `Open tmux Window`: `select-window` して window context attach
3. window context attach 中も pane 単位入力は active pane に送る

## 6.2 Create / Kill semantics

1. `create new pane` は session working dir を起点に作成
2. 作成中は session row を spinner 表示
3. 作成完了 ack 後に新 pane を自動選択して attach
4. kill は optimistic remove + tombstone で復活ちらつきを防止
5. server confirm 失敗時のみ rollback

## 6.3 DnD Layout Editing

DnD で次を操作可能にする。

1. pane reorder（sidebar manual stable order）
2. tmux layout mutate（window内/跨ぎ）

drop target:

1. `left`
2. `right`
3. `top`
4. `bottom`
5. `tab` (swap/stack semantics)
6. `new-window`

tmux command mapping:

1. `swap-pane`
2. `move-pane`
3. `join-pane`
4. `break-pane`
5. `resize-pane`
6. `select-layout`

mutation contract:

1. `mutation_id` 必須
2. per-window lock
3. optimistic preview
4. commit ack で確定
5. nack/timeout で deterministic rollback

## 7. Architecture (Rust, WezTerm family)

## 7.1 Components

1. `agtmuxd-rs` (daemon)
2. `agtmux-desktop` (WezTerm-based desktop app)
3. `agtmux-cli` (ops/debug)
4. `agtmux-agent-adapters` (provider adapters)
5. `agtmux-store` (SQLite state store)
6. `agtmux-protocol` (binary frame codec + contract types)

## 7.2 Plane separation

Data plane:

1. pane output bytes stream
2. high-priority selected pane lane

Control plane:

1. topology sync
2. attach/focus/write/resize/create/kill
3. layout mutate API
4. state/attention events

## 7.3 Selected pane output policy

1. local: `pipe-pane -O` primary
2. ssh: remote tap bridge primary
3. control bridge output is fallback only
4. selected pane で snapshot 取得しない

## 7.4 Adopted technical profile (from rust rewrite proposal)

この Phase28 は次の技術選定を採用する。

1. 言語: Rust（daemon/desktop/cli を統一）
2. terminal state engine: `wezterm-term`（pinned commit）
3. terminal type/escape helpers: `termwiz`
4. async runtime: `tokio`
5. persistence: `rusqlite`（WAL）
6. render host: WezTerm系 UI stack（fork or thin integration）

採用理由:

1. cursor/IME/scroll の再実装コストを最小化
2. data-plane hot path を raw bytes で通しやすい
3. PoCで破綻した snapshot 再構成型の回帰を防ぎやすい

## 7.5 Workspace / crate structure

```text
agtmux-rs/
  Cargo.toml
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
    agtmux-desktop/
  docs/
    architecture/
    implementation-records/
    runbooks/
```

設計ルール:

1. `agtmux-daemon` は `agtmux-protocol` と `agtmux-state` にのみ依存
2. provider固有実装は `agtmux-agent-adapters` に隔離
3. desktop は tmux command を直接叩かず daemon 経由に固定

## 7.6 Dependency risk controls

1. `wezterm-term` は tag ではなく commit pin
2. pin 更新は「隔週ウィンドウ」でのみ実施
3. pin 更新時は replay regression（codex/claude/gemini traces）を必須実行
4. 失敗時は即時 pin rollback

## 8. Protocol v3 (binary-first)

## 8.1 Transport

1. local: UDS
2. framing: length-prefixed binary
3. per-pane monotonic sequence

## 8.2 Core Frames

1. `hello`, `hello_ack`
2. `topology_sync`, `topology_delta`
3. `attach`, `attached`
4. `focus`, `write`, `resize`, `ack`
5. `output` (raw bytes, seq, pane_ref, source)
6. `state` (presence/activity/attention/confidence)
7. `layout_mutate`, `layout_preview`, `layout_commit`, `layout_revert`
8. `error`, `metrics`

## 8.3 Frame Contracts

1. `output.source in {pane_tap, bridge_fallback, preview}`
2. `layout_*` は idempotent (`mutation_id`)
3. stale action は `pane_epoch` 不一致で fail-closed
4. ack は `request_id` を必須返却

## 8.4 Contract extensions for window-capable UX

1. `topology_*` には `window_ref`, `window_order`, `window_layout_hash` を含める
2. `open tmux window` は `focus` frame で `focus_level=window|pane` を持つ
3. `layout_mutate` は `target_window_ref` を必須にし、window跨ぎ mutate を明示
4. `layout_commit/revert` は server-side resolved topology snapshot を返す

## 9. Stream / Mutation FSM

## 9.1 Pane stream FSM

1. `detached`
2. `recovering`
3. `live`

rules:

1. `recovering -> live` は first output 受信でのみ遷移
2. `live` で snapshot-like frame 受理禁止
3. stream broken 時は `recovering` に戻し、再attachを試行
4. timeout 超過で UI に degraded badge 表示

## 9.2 Layout mutation FSM

1. `idle`
2. `previewing`
3. `pending_commit`
4. `committed`
5. `reverted`

rules:

1. lock取得前に preview 開始しない
2. timeout 時は必ず revert
3. client/server とも `mutation_id` で重複実行を抑止

## 10. State Engine / Attention Engine

## 10.1 Canonical state

Presence:

1. `managed`
2. `unmanaged`
3. `unknown`

Activity:

1. `running`
2. `waiting_input`
3. `waiting_approval`
4. `idle`
5. `error`
6. `unknown`

Attention:

1. `task_complete`
2. `waiting_input`
3. `approval`
4. `error`
5. `none`

## 10.2 Source priority

1. hooks
2. wrapper events
3. adapter session metadata (`/resume` 相当)
4. runtime output heuristic

## 10.3 Last-active policy

1. managed pane: provider session last event time を優先
2. unmanaged pane: last-active を表示しない（ノイズ削減）
3. fallback 時のみ tmux activity timestamp を使う

## 10.4 Title policy

1. user rename を最優先
2. provider session title（codex/claude resume由来）
3. provider transcript excerpt
4. tmux metadata fallback

## 10.5 Attention UX policy

1. `running -> idle` だけで attention を立てない
2. actionable のみキュー表示
3. queue と list order を分離
4. unread/ack を保持

## 11. Multi-target / SSH Design

1. target isolation（障害隔離）
2. persistent ssh connection pooling
3. target health state: `ok | degraded | down`
4. read path は partial result 継続
5. write path は target-scoped error 明示
6. local target は ssh 障害の影響を受けない

## 12. Performance Budget / Observability

## 12.1 SLO

1. local input p95 < 20ms
2. local active fps median >= 55
3. ssh input p95 < 80ms
4. pane switch p95 < 120ms
5. layout mutate commit p95 < 250ms (local)
6. selected pane stream gap p95 < 40ms
7. desktop CPU < 25% (active single pane baseline)

## 12.2 Required Metrics

1. `fps_current`, `fps_p50`, `fps_p95`
2. `input_latency_ms_p50/p95`
3. `stream_latency_ms_p50/p95`
4. `selected_hotpath_capture_count`
5. `stream_source_ratio{pane_tap,bridge_fallback,preview}`
6. `layout_mutation_success_rate`
7. `state_source_ratio{hook,wrapper,adapter,heuristic}`
8. `window_focus_switch_latency_ms`
9. `selected_stream_gap_ms_p50/p95`
10. `desktop_cpu_percent`, `desktop_mem_mb`

## 12.3 Gate

CI gate で以下を merge 条件にする。

1. SLO 逸脱なし
2. selected pane capture count = 0
3. cursor/scroll/IME replay suite pass
4. window focus attach E2E pass
5. layout mutation rollback E2E pass

## 13. Test Strategy

## 13.1 Unit

1. protocol codec
2. stream/mutation FSM
3. adapter state merge priority
4. attention rule engine

## 13.2 Integration

1. attach/focus/write/resize/create/kill
2. open pane / open tmux window context menu actions
3. layout mutate optimistic + rollback
4. ssh degraded/down behavior
5. session accordion + organize mode switch under active stream
6. filter tabs (`all|managed|attention|pinned`) consistency

## 13.3 Replay / E2E

1. codex trace
2. claude trace
3. gemini trace
4. CJK/IME scenarios
5. long-running stream stress
6. window split/join/swap/break replay scenarios

## 14. Implementation Plan (Phase 28 breakdown)

## 28-A: Workspace bootstrap

1. Rust workspace skeleton
2. protocol v3 codec tests
3. dependency pin policy + update playbook

Gate:

1. codec round-trip pass
2. dependency lock reproducibility pass

## 28-B: tmux stream core

1. pane tap manager
2. control bridge fallback
3. selected pane stream-only contract

Gate:

1. selected pane hotpath capture count = 0

## 28-C: terminal integration

1. WezTerm terminal surface integration
2. attach/focus/write/resize lifecycle
3. local echo off path
4. IME preedit/commit native path verification

Gate:

1. cursor/scroll regression suite pass
2. IME regression suite pass

## 28-D: sidebar IA v2

1. session block + filter tabs + organize menu
2. pane row title/time/state rules
3. context menus (`Open Pane`, `Open tmux Window`)

Gate:

1. no reorder jitter under active updates

## 28-E: window-capable UX

1. window-grouped view
2. window context open flow
3. window metadata summaries

Gate:

1. session/window/pane 行き来で誤attach 0

## 28-F: layout editing

1. DnD targets + mutation API
2. optimistic + rollback + lock/idempotency

Gate:

1. race replay で topology divergence 0

## 28-G: state/attention engine

1. adapter registry
2. codex/claude/gemini adapters
3. actionable attention queue

Gate:

1. status precision / attention precision budget pass

## 28-H: multi-target hardening + release gate

1. ssh pooling/health/backoff
2. perf budget CI gates
3. packaging/runbook
4. dependency update rollback drill

Gate:

1. local/ssh mixed workload E2E pass
2. pin update failure rollback dry-run pass

## 15. Risks and Mitigations

1. WezTerm fork drift
2. tmux command race in layout mutate
3. adapter drift against CLI updates
4. ssh jitter on remote taps
5. terminal stack update regressions
6. over-engineering by premature custom rendering

Mitigation:

1. fork surface minimization
2. per-window lock + mutation_id
3. contract tests with recorded fixtures
4. target isolation and retry policy
5. commit pin + replay regression gate
6. custom renderer は v2 scope outside（必要時のみ ADR で解禁）

## 15.1 Explicit non-adoption list (important)

今回の Phase28 では、別案のうち次は採用しない。

1. `iced + custom wgpu terminal renderer` の初期導入
2. daemon/client 両方で terminal state を二重保持する設計
3. early-phase での UI 完全自由化（terminal correctness を犠牲にするため）

採用しない理由:

1. 先に安定動作と運用価値（tmux操作 + state精度）を固めるほうが成功確率が高い
2. custom renderer は後からでも追加可能だが、逆は難しい

## 16. Definition of Done (Phase 28)

1. tmux-first invariants を破らない
2. pane-first default UX と window-capable UX の両立
3. selected stream-only が計測で担保される
4. cursor/IME/scroll replay を安定通過
5. docs/decision/runbook が揃っている

## 17. Final Recommendation

この設計で再始動する。  
ただし実装順は「terminal correctness先行」ではなく、`tmux operations UX` と `state accuracy` を同時に進める。  
理由は、AGTMUX の価値が terminal 単体性能ではなく「tmux運用の判断速度」にあるため。
