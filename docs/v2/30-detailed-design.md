# AGTMUX v2 Detailed Design and Implementation Plan

Date: 2026-02-21  
Status: Draft v1  
Depends on: `./10-product-charter.md`, `./20-unified-design.md`

## 1. Objective

目的は、次の2点を同時に満たすこと。

1. terminal correctness（cursor/IME/scroll）を安定化
2. tmux operations UX（pane主導 + window操作）を高速化

## 2. Architectural Decisions (Locked)

1. UI host は `wezterm-gui fork` 一本
2. daemon/client 間は binary protocol v3
3. selected pane は stream-only
4. tmux は system of record（layout source of truth）
5. state/attention は adapter-first

## 3. Workspace and Package Layout

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
```

## 3.1 Fork Integration Boundary

fork 実装は次を固定する（詳細正本: `./specs/74-fork-surface-map.md`）。

1. fork 側 `wezterm-gui` を renderer/input host として利用する
2. AGTMUX UX（sidebar/menu/DnD）は fork repo の allowed zones に実装する
3. daemon 接続は fork repo の integration layer に実装する
4. `wezterm mux` は置換せず、desktop 側で projection model を構築する
5. tmux topology は daemon 由来イベントのみを採用する
6. `termwiz` / `wezterm-term` / parser core / mux core の変更は ADR 必須

Runtime bridge 分担:

1. `AgtmuxRuntimeBridge`: protocol v3 の encode/decode, attach session lifecycle
2. `TerminalFeedRouter`: paneごとの output seq 適用と stale frame drop
3. `InputRouter`: write/resize/focus を daemon API に変換
4. `TopologyProjectionStore`: session/window/pane の UI 投影を管理

fork 実体の取り込み方式は `ADR-0005`、改造範囲は `specs/74` を正本とする。

## 4. Data Model

## 4.1 Logical Entities

1. Target
2. Session
3. Window
4. Pane
5. Runtime
6. RuntimeState
7. AttentionItem
8. LayoutMutation

## 4.2 Relational Schema (SQLite)

```sql
CREATE TABLE targets (
  target_id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('local','ssh')),
  connection_ref TEXT NOT NULL DEFAULT 'local',
  health TEXT NOT NULL CHECK(health IN ('ok','degraded','down')),
  last_seen_at TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE sessions (
  target_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  session_name TEXT NOT NULL,
  manual_order INTEGER NOT NULL DEFAULT 0,
  pinned INTEGER NOT NULL DEFAULT 0,
  collapsed INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (target_id, session_id),
  FOREIGN KEY (target_id) REFERENCES targets(target_id) ON DELETE CASCADE
);

CREATE TABLE windows (
  target_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  window_name TEXT NOT NULL,
  window_index INTEGER NOT NULL,
  layout_hash TEXT NOT NULL DEFAULT '',
  manual_order INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (target_id, session_id, window_id),
  FOREIGN KEY (target_id, session_id) REFERENCES sessions(target_id, session_id) ON DELETE CASCADE
);

CREATE TABLE panes (
  target_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  pane_id TEXT NOT NULL,
  pane_epoch INTEGER NOT NULL DEFAULT 0,
  pane_index INTEGER NOT NULL DEFAULT 0,
  tmux_title TEXT NOT NULL DEFAULT '',
  user_title TEXT NOT NULL DEFAULT '',
  current_cmd TEXT NOT NULL DEFAULT '',
  current_path TEXT NOT NULL DEFAULT '',
  managed INTEGER NOT NULL DEFAULT 0,
  pinned INTEGER NOT NULL DEFAULT 0,
  manual_order INTEGER NOT NULL DEFAULT 0,
  last_tmux_activity_at TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (target_id, session_id, window_id, pane_id),
  FOREIGN KEY (target_id, session_id, window_id) REFERENCES windows(target_id, session_id, window_id) ON DELETE CASCADE
);

CREATE TABLE runtimes (
  runtime_id TEXT PRIMARY KEY,
  target_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  pane_id TEXT NOT NULL,
  provider TEXT NOT NULL CHECK(provider IN ('codex','claude','gemini','unknown')),
  source TEXT NOT NULL CHECK(source IN ('hook','wrapper','adapter','heuristic')),
  title TEXT NOT NULL DEFAULT '',
  last_event_at TEXT,
  confidence REAL NOT NULL DEFAULT 0.0,
  FOREIGN KEY (target_id, session_id, window_id, pane_id) REFERENCES panes(target_id, session_id, window_id, pane_id) ON DELETE CASCADE
);

CREATE TABLE runtime_states (
  runtime_id TEXT PRIMARY KEY,
  presence TEXT NOT NULL CHECK(presence IN ('managed','unmanaged','unknown')),
  activity TEXT NOT NULL CHECK(activity IN ('running','waiting_input','waiting_approval','idle','error','unknown')),
  attention TEXT NOT NULL CHECK(attention IN ('task_complete','waiting_input','approval','error','none')),
  unread INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (runtime_id) REFERENCES runtimes(runtime_id) ON DELETE CASCADE
);

CREATE TABLE layout_mutations (
  mutation_id TEXT PRIMARY KEY,
  target_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','committed','reverted','failed')),
  requested_by TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  topology_snapshot_json TEXT NOT NULL,
  error_text TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE attention_events (
  event_id TEXT PRIMARY KEY,
  runtime_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  read INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  FOREIGN KEY (runtime_id) REFERENCES runtimes(runtime_id) ON DELETE CASCADE
);
```

## 4.3 Data Invariants

1. `pane_epoch` 不一致の write/mutate は拒否
2. `runtime_states` は state engine のみ書き込み可能
3. `layout_mutations` は必ず terminal topology snapshot を持つ
4. `manual_order` は user action でのみ更新
5. `targets.connection_ref` は `local` target では `local` を使う
6. `targets.connection_ref` は `ssh` target では接続プロファイルIDを使う

## 5. Protocol v3 Detailed Contract

wire-level fixed spec は `./specs/70-protocol-v3-wire-spec.md` を正本とする。

## 5.1 Transport

1. UDS (local) / tunneled UDS (ssh)
2. length-prefixed binary frame
3. max frame size = 1MB

## 5.2 Command Frames

1. `focus {focus_level, pane_ref|window_ref}`
2. `attach {pane_ref}`
3. `write {pane_ref, pane_epoch, bytes}`
4. `resize {pane_ref, cols, rows}`
5. `create_pane {session_ref, cwd_hint}`
6. `kill_pane {pane_ref, signal}`
7. `rename_pane {pane_ref, title}`
8. `layout_mutate {mutation_id, op, source_ref, target_ref}`

## 5.3 Event Frames

1. `topology_sync`
2. `topology_delta`
3. `output {pane_ref, seq, source, bytes}`
4. `state {runtime_id, presence, activity, attention, confidence}`
5. `layout_preview/commit/revert`
6. `error`
7. `metrics`

## 5.4 Ordering Rules

1. `output.seq` は pane ごと単調増加
2. `ack` は常に `request_id` を返す
3. stale epoch は fail-closed (`E_STALE_PANE`)
4. mutation timeout は server 主導で `layout_revert`

## 6. Core Flows

## 6.1 Open Pane

1. UI selects pane row
2. daemon: `select-window -> select-pane`
3. daemon attaches stream actor
4. first output 到着で stream state を `live` に更新
5. UI terminal surfaces live stream

## 6.2 Open tmux Window

1. context menu `Open tmux Window`
2. daemon focuses window
3. daemon emits window topology snapshot
4. UI switches to window context view

## 6.3 Create New Pane

1. UI sends `create_pane(session_ref, cwd_hint)`
2. session row spinner on
3. daemon creates pane in session cwd
4. topology delta with new pane
5. UI auto-select new pane, spinner off

## 6.4 Kill Pane

1. UI optimistic remove with tombstone
2. daemon sends `kill-pane`
3. committed on ack
4. restore only on explicit failure

## 6.5 DnD Layout Mutation

1. UI creates `mutation_id`
2. daemon acquires per-window lock
3. applies tmux command set
4. emits `layout_commit` or `layout_revert`
5. UI finalizes or rolls back preview

## 7. State Engine

## 7.1 Adapter Contract

1. `detect(pane_ctx) -> provider, confidence`
2. `poll(runtime_ref) -> runtime_snapshot`
3. `ingest(event) -> state_transition`
4. `extract_title(snapshot|event) -> title`
5. `extract_last_active(snapshot|event) -> timestamp`

## 7.2 Priority

1. hooks
2. wrapper events
3. provider-specific adapter snapshot
4. heuristic parser

## 7.3 Attention Rules

1. running->idle だけでは発火しない
2. actionable event のみ unread queue
3. ack で unread=0
4. queue sort は created_at desc

## 8. Performance and Reliability Plan

定量gateの正本は `./specs/71-quality-gates.md` を参照。

## 8.1 Targets

1. local input p95 < 20ms
2. local fps median >= 55
3. selected stream gap p95 < 40ms
4. pane switch p95 < 120ms

## 8.2 Instrumentation

1. daemon exports per-pane output gap metrics
2. desktop exports frame pacing and input latency
3. trace IDs in request/ack/error for debugging

## 8.3 Failure handling

1. stream broken -> recovering -> reattach
2. target down -> partial results continue
3. mutation timeout -> auto revert

## 9. Implementation Plan (Concrete)

## Phase A: Foundation (1 week)

1. workspace bootstrap
2. protocol crate with unit tests
3. store crate + migrations

## Phase A1: Fork Hook Map Spike (2-3 days)

1. `specs/75-fork-hook-map-spike.md` 成果物を作成
2. file/function-level hook points を確定
3. hook map を Phase C の実装入力として固定

Exit criteria:

1. protocol round-trip test pass
2. schema migration repeatability pass

## Phase B: Stream Core (1-2 weeks)

1. pane tap manager
2. control bridge fallback
3. selected stream-only enforcement

Exit criteria:

1. selected hotpath capture count == 0
2. open pane latency p95 < 150ms (local)

## Phase C: Desktop Terminal (1-2 weeks)

1. wezterm-gui fork integration skeleton
2. runtime bridge (`AgtmuxRuntimeBridge`, `TerminalFeedRouter`, `InputRouter`)
3. attach/focus/write/resize path
4. IME basic preedit/commit
5. allowed/restricted path CI check導入

Exit criteria:

1. cursor/scroll/IME replay tests pass
2. quality gate の Dev stage pass
3. restricted zone 変更が 0（または ADR 付きのみ）

## Phase H: Output Hotpath Decision (2-3 days)

1. `specs/76-output-hotpath-framing-policy.md` に従い計測
2. MessagePack 維持 or `output_raw` 導入を ADR で決定
3. protocol spec と回帰計測結果を更新

## Phase D: Sidebar/Window UX (1 week)

1. session blocks + filters + organize
2. context menus
3. window-grouped mode

Exit criteria:

1. no reorder jitter
2. `Open Pane` / `Open tmux Window` E2E pass

## Phase E: Layout Editing (1 week)

1. DnD drop targets
2. mutation API + lock + rollback
3. conflict tests

Exit criteria:

1. topology divergence = 0 in replay

## Phase F: State/Attention (1 week)

1. adapter registry
2. codex/claude/gemini adapters
3. attention queue

Exit criteria:

1. quality gate の Beta stage（state/attention）pass

## Phase G: Multi-target Hardening (1 week)

1. ssh pooling/backoff
2. target isolation
3. runbooks

Exit criteria:

1. local unaffected under ssh failure
2. quality gates pass (`./specs/71-quality-gates.md`)

## 10. Test Matrix

## 10.1 Unit

1. protocol codec
2. state transitions
3. mutation planner

## 10.2 Integration

1. create/kill/rename pane
2. open pane/window actions
3. DnD mutation + rollback

## 10.3 E2E Replay

1. codex interactive trace
2. claude interactive trace
3. gemini interactive trace
4. CJK/IME input trace

## 10.4 UI Feedback Loop

1. GUIセッション実行のみ許可（SSH実行禁止）
2. `AGTMUX_RUN_UI_TESTS=1` opt-in でUI testsを起動
3. `scripts/ui-feedback/run-ui-feedback-report.sh` で loop 実行とmarkdown artifactを生成
4. report の `tests_failures=0` を UI変更PR の必須条件にする
5. AX列挙の環境揺れは `window visible` 確認後に skip 許容
6. `ui_snapshot_errors` は fail ではなく診断指標として集計

## 11. Decision Resolution (ADR)

1. fork branch strategy: `./adr/ADR-0001-wezterm-fork-branch-strategy.md`
2. ssh framing: `./adr/ADR-0002-ssh-tunnel-framing.md`
3. notification scope: `./adr/ADR-0003-notification-scope.md`
4. fork integration boundary: `./adr/ADR-0004-wezterm-fork-integration-boundary.md`
5. fork source integration model: `./adr/ADR-0005-fork-source-integration-model.md`

## 12. Definition of Ready (Before coding)

1. invariant checklist accepted
2. protocol v3 draft fixed
3. schema migration test ready
4. first E2E fixture prepared
5. UI feedback loop script set ready (`scripts/ui-feedback/run-ui-tests.sh`, `scripts/ui-feedback/run-ui-loop.sh`, `scripts/ui-feedback/run-ui-feedback-report.sh`)
6. workspace bootstrap path fixed (`./specs/72-bootstrap-workspace.md`)
7. fork surface map confirmed (`./specs/74-fork-surface-map.md`)
8. fork hook map spike completed (`./specs/75-fork-hook-map-spike.md`)
