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
```

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
  connection_ref TEXT NOT NULL,
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
  confidence REAL NOT NULL DEFAULT 0.0
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

## 5. Protocol v3 Detailed Contract

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

1. wezterm-gui fork integration
2. attach/focus/write/resize path
3. IME basic preedit/commit

Exit criteria:

1. cursor/scroll/IME replay tests pass
2. local input p95 < 25ms

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

1. status precision budget pass
2. attention precision budget pass

## Phase G: Multi-target Hardening (1 week)

1. ssh pooling/backoff
2. target isolation
3. runbooks

Exit criteria:

1. local unaffected under ssh failure

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
3. `run-ui-feedback-report.sh` で loop 実行とmarkdown artifactを生成
4. report の `tests_failures=0` を UI変更PR の必須条件にする
5. AX列挙の環境揺れは `window visible` 確認後に skip 許容
6. `ui_snapshot_errors` は fail ではなく診断指標として集計

## 11. Open Decisions (Need explicit ADR)

1. wezterm fork branch strategy (long-lived vs rebase windows)
2. ssh tunnel framing detail
3. notification channel scope (app only vs external webhook)

## 12. Definition of Ready (Before coding)

1. invariant checklist accepted
2. protocol v3 draft fixed
3. schema migration test ready
4. first E2E fixture prepared
5. UI feedback loop script set ready (`run-ui-tests.sh`, `run-ui-loop.sh`, `run-ui-feedback-report.sh`)
