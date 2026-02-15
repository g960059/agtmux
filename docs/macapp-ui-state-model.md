# macOS App State Model and UI Categorization (Session-Pane First)

Status: Draft  
Last Updated: 2026-02-14  
Scope: `AGTMUXDesktop` (`macapp/`) + daemon/API read model alignment

## 1. Problem Statement

For agent-driven development, tmux is mainly a persistence/transport layer.  
Operationally, users care about `session` and `pane` first, not strict tmux hierarchy.

Current pain points:

- `completed` vs `idle` is ambiguous.
- agent-less panes are mixed with agent states.
- completion toast can be missed.
- forcing `window` as mandatory hierarchy adds UI noise for large displays.

Operator intent is:

1. Is an agent actively running?
2. Is user interaction required?
3. What finished and still needs review?

## 2. Workflow Assumptions

This document assumes the common development pattern:

- `1 worktree/branch = 1 session`
- `1 pane = 1 agent` or `1 user utility pane` (editor, git, logs)
- `window` is optional, used only when pane count grows and manual grouping helps

Therefore the primary model is `target > session > pane`, with `window` as optional metadata.

## 3. Canonical Object Model

- Primary:
  - `target`
  - `session`
  - `pane`
- Optional:
  - `window` (group label for panes, not required for core UI flow)

Rules:

- Any feature must work when window grouping is disabled.
- Any pane can be rendered with only session context.
- `window_id/window_name` may be absent in UI view-model if flattened mode is selected.

## 4. Two-Axis Pane State Model

## 4.1 `agent_presence`

- `managed`: pane has a recognized managed agent runtime.
- `none`: pane has no managed agent runtime.
- `unknown`: detection unavailable or stale/inconclusive.

`none` is not `idle`.

## 4.2 `activity_state` (meaningful only when `agent_presence=managed`)

- `running`
- `waiting_input`
- `waiting_approval`
- `idle`
- `error`
- `unknown`

## 4.3 Derived `display_category` (UI-level)

- `attention`: `waiting_input`, `waiting_approval`, `error`
- `running`
- `idle`
- `unmanaged`: `agent_presence=none`
- `unknown`

## 5. Completion and Review Queue

`completed` should not be a long-lived pane state in primary UI.

- Keep completion as event: `task_completed`.
- Persist events in `review queue` until user acknowledges.
- Toast is optional secondary feedback only.

Queue item fields:

- `queue_item_id`
- `kind` (`task_completed`, `needs_input`, `needs_approval`, `error`)
- `target`, `session_name`, `pane_id`
- `window_id` (optional)
- `runtime_id`
- `created_at`
- `summary`
- `unread`
- `acknowledged_at` (nullable)
- `action_id` (nullable)

## 6. State Transition Policy

Core transitions:

- `none -> managed+idle` on agent attach/runtime detect
- `managed+idle -> managed+running` on task start
- `managed+running -> managed+waiting_input` on prompt/input request
- `managed+running -> managed+waiting_approval` on approval gate
- `managed+waiting_* -> managed+running` on user action
- `managed+running -> managed+idle` on task completion
  - emit one `task_completed` queue item
- `managed+* -> managed+error` on failure/crash
- `managed+* -> none` on agent detach
- any `* -> unknown` on stale/inconclusive detection

## 7. Aggregation Rules (Session First)

Session is the primary aggregate unit.

Category precedence:

1. `attention`
2. `running`
3. `idle`
4. `unmanaged`
5. `unknown`

Required counters:

- `attention_count`
- `running_count`
- `idle_count`
- `unmanaged_count`
- `unknown_count`
- `review_unread_count`

Window rollup is optional:

- enabled when user selects `group_by_window=on|auto`
- disabled by default for pane-light sessions

## 8. macOS UI Modes

## 8.1 By Session (default, topology-first)

- Session tiles/cards are top-level objects.
- Each session directly lists panes by default.
- Optional nested window section when grouping is enabled.
- Pane card shows:
  - `display_category` badge
  - compact state reason (`waiting_input`, `approval`, `error`)
  - optional window label

## 8.2 By Status (urgency-first)

Columns:

- `attention`
- `running`
- `idle`
- `unmanaged`
- `unknown` (toggle)

Pane cards always include session label.
Window label is optional and can be hidden.

## 8.3 Review Queue (persistent)

- Global queue panel for unresolved events.
- Filter by `session`, `kind`, `unread`.
- Open-pane and acknowledge actions.
- Badge counts shown on session and global header.

## 8.4 Settings

- view mode default (`by_session` or `by_status`)
- group by window (`off|auto|on`)
- show/hide window metadata
- show/hide session metadata in status view
- compact card mode
- hide unmanaged column
- sort priority (`attention-first` vs `session-first`)

## 9. Notification Policy

Toast policy:

- `waiting_input`: toast (+ optional sound)
- `waiting_approval`: toast (+ optional sound)
- `error`: sticky banner/toast
- `task_completed`: non-sticky toast (optional)

Anti-noise:

- dedupe same pane+kind within 30s
- no sound if pane/session is focused
- suppress short flapping (`running <-> idle`)

Durable truth is `review queue`, not toast timeout.

## 10. API Extension Proposal (v1 additive)

Pane item additive fields:

```json
{
  "agent_presence": "managed|none|unknown",
  "activity_state": "running|waiting_input|waiting_approval|idle|error|unknown",
  "display_category": "attention|running|idle|unmanaged|unknown",
  "needs_user_action": true,
  "review_unread_count": 2,
  "last_completed_at": "2026-02-14T13:00:00Z",
  "window": {
    "window_id": "@3",
    "window_name": "backend",
    "present": true
  }
}
```

Session summary additive fields:

```json
{
  "project_ref": {
    "worktree": "exp/go-codex-implementation-poc",
    "branch": "exp/go-codex-implementation-poc"
  },
  "top_category": "attention",
  "by_category": {
    "attention": 1,
    "running": 2,
    "idle": 4,
    "unmanaged": 3,
    "unknown": 0
  },
  "review_unread_count": 5
}
```

Queue endpoints (proposed):

- `GET /v1/review/queue`
- `POST /v1/review/queue/{id}/ack`
- `POST /v1/review/queue/ack-all`

## 11. Migration Plan

Phase A (compatibility):

- keep existing canonical `state` response
- derive `display_category` in app
- introduce local review queue storage + unread badges

Phase B (backend enrichment):

- add `agent_presence` and `activity_state` fields
- add session-level `by_category` and `review_unread_count`
- add optional window metadata object on pane

Phase C (session-pane-first UI):

- default UI to session > pane
- move window grouping behind setting (`off|auto|on`)
- remove `completed` from primary columns

Phase D (queue-first completion UX):

- completion visible via queue until acknowledgment
- toast remains optional helper only

## 12. Acceptance Criteria

1. Pane with `agent_presence=none` is never shown as `idle`.
2. Primary UI remains fully functional with window grouping disabled.
3. Operator can switch between `By Session` and `By Status`.
4. Completion is visible until acknowledged (not toast-dependent).
5. Session labels are sufficient to triage without window metadata.
6. Existing CLI/API consumers remain backward compatible during migration.
