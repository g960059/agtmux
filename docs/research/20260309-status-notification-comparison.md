# Research: cmux, CodexMonitor, OpenAI Codex Status + Notification Comparison

**Date**: 2026-03-09  
**Status**: Complete  
**Scope**: Compare how `cmux`, `CodexMonitor`, and OpenAI's `codex` model status, trigger user-facing attention, and clear/dismiss that attention.

## Sources

Repository snapshots inspected on 2026-03-09:

- `manaflow-ai/cmux` @ `a636104fb92a59655a658d3b22909f0ae4f2d8e2`
- `Dimillian/CodexMonitor` @ `a70b91a37bb6d6eff49f5021ca20ae06623ac52e`
- `openai/codex` @ `2bc3e52a91bb88a0e067a95f8f8559f8711d30e6`

Primary files:

- cmux:
  - `https://github.com/manaflow-ai/cmux/blob/a636104fb92a59655a658d3b22909f0ae4f2d8e2/CLI/cmux.swift`
  - `https://github.com/manaflow-ai/cmux/blob/a636104fb92a59655a658d3b22909f0ae4f2d8e2/docs/notifications.md`
  - `https://github.com/manaflow-ai/cmux/blob/a636104fb92a59655a658d3b22909f0ae4f2d8e2/tests/test_notifications.py`
  - `https://github.com/manaflow-ai/cmux/blob/a636104fb92a59655a658d3b22909f0ae4f2d8e2/tests/test_focus_notification_dismiss.py`
- CodexMonitor:
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/utils/threadStatus.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/utils/threadStatus.test.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/features/threads/hooks/useThreadTurnEvents.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/features/threads/hooks/useThreadItemEvents.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/features/notifications/hooks/useAgentResponseRequiredNotifications.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/features/notifications/hooks/useAgentResponseRequiredNotifications.test.tsx`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/features/notifications/hooks/useAgentSystemNotifications.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src/services/tauri.ts`
  - `https://github.com/Dimillian/CodexMonitor/blob/a70b91a37bb6d6eff49f5021ca20ae06623ac52e/src-tauri/src/notifications.rs`
- OpenAI Codex:
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/README.md`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/docs/config.md`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/app-server/src/thread_status.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/app-server/tests/suite/v2/thread_status.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/app-server-protocol/schema/json/v2/ThreadStatusChangedNotification.json`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/app-server-protocol/schema/typescript/AgentStatus.ts`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/hooks/src/user_notification.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/notifications/mod.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/notifications/osc9.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/notifications/bel.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/bottom_pane/pending_thread_approvals.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/bottom_pane/pending_input_preview.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/codex-rs/tui/src/bottom_pane/approval_overlay.rs`
  - `https://github.com/openai/codex/blob/2bc3e52a91bb88a0e067a95f8f8559f8711d30e6/docs/tui-request-user-input.md`
  - Official config reference entry point:
    - `https://developers.openai.com/codex/config-reference`

## Executive Summary

The three systems split into two different design families.

- `cmux` treats status and notifications as separate primitives, but its shipped agent-specific state machine is Claude-focused. It has a generic sidebar status API, a notification panel with read/unread state, and Claude hook glue that collapses multiple attention states into a single persistent status: `Needs input`.
- `CodexMonitor` has a richer in-app state model than its desktop notifications. Its thread row dot is a small UI-class mapping, while system notifications are split into:
  - immediate "response required" attention for approvals and user-input requests
  - delayed "success/error" notifications for completed turns
- OpenAI `codex` exposes the cleanest protocol-level status vocabulary:
  - thread-level state: `notLoaded`, `idle`, `systemError`, `active(activeFlags)`
  - active flags: `waitingOnApproval`, `waitingOnUserInput`
  - separate agent-level lifecycle: `pending_init`, `running`, `completed`, `errored`, `shutdown`, `not_found`
  - but its open-source repo does **not** show a built-in desktop-notification path for waiting states; the built-in `notify` hook is completion-oriented.

The practical implication for `agtmux` is that status and notification policy should stay separate:

- persistent status should preserve `running`, `waiting_input`, `waiting_approval`
- "attention" notifications should trigger on `waiting_input` and `waiting_approval`
- completion notifications should be a different policy surface from attention notifications
- notification clearing should be keyed by request identity or focus/consume events, not by a blind `idle`

## Comparison Table

| System | Persistent status vocabulary | Waiting states distinct? | Completion signal | Attention notification policy | Clear / dismiss model |
|---|---|---|---|---|---|
| `cmux` | Generic sidebar status pills; shipped Claude helper writes `Running` or `Needs input` | Partially. Notification subtitle distinguishes `Permission` vs `Waiting`, but persistent status collapses both to `Needs input` | Claude `stop` hook emits `Completed`; Codex docs show `notify` hook on turn complete only | Immediate panel/system notification for Claude `notification`; Codex sample only covers turn-complete | Notification panel has read/unread. Focus marks read. `prompt-submit` clears queued notifications. `stop` clears Claude sidebar status |
| `CodexMonitor` | `processing`, `reviewing`, `unread`, `ready`; home labels `Running`, `Reviewing`, `Idle` | Partially. `requestUserInput` becomes `unread`; review is distinct; approval is mostly toast/queue, not a dedicated dot state | `turn/completed` and `agentMessageCompleted` can trigger success notification | Immediate attention notifications for approval requests and questions; success/error notifications are delayed and separate | System notifications use `autoCancel`; dedupe keys are removed when request disappears; no read/unread notification store |
| OpenAI `codex` | Thread status: `notLoaded`, `idle`, `systemError`, `active(activeFlags)`; Agent status: `pending_init`, `running`, `completed`, `errored`, `shutdown`, `not_found` | Yes, in thread active flags: `waitingOnApproval`, `waitingOnUserInput` | `turn/completed` and legacy `agent-turn-complete` notify payload | Open-source repo confirms completion-oriented notify hook; no direct evidence of built-in waiting-state desktop notifications | Status flags clear automatically when guards resolve or turns complete; TUI desktop notifications are fire-and-forget OSC 9 / BEL |

## 1. cmux

### Confirmed

`cmux` exposes two separate primitives:

- generic sidebar status entries via `set-status`, `clear-status`, `list-status`
- notification items via `notify`, `list-notifications`, `clear-notifications`

The generic status API is not agent-specific. The shipped agent-specific helper in `CLI/cmux.swift` is `claude-hook`, not `codex-hook`.

Claude hook behavior:

- `session-start` / `active`
  - writes sidebar status `claude_code = Running`
  - icon: `bolt.fill`
- `notification` / `notify`
  - parses the incoming hook payload
  - classifies it as `Permission`, `Error`, `Waiting`, or `Attention`
  - sends a notification item to the notification panel
  - sets persistent sidebar status `claude_code = Needs input`
- `prompt-submit`
  - clears queued notifications for the workspace
  - restores sidebar status to `Running`
- `stop` / `idle`
  - clears the Claude sidebar status
  - optionally emits a `Completed` notification, enriched from transcript or stored session context

Classification logic in `classifyClaudeNotification(...)` is keyword-based:

- approval-like text -> `Permission`
- error-like text -> `Error`
- idle / wait / input / prompt -> `Waiting`
- anything else -> `Attention`

This is a notable design choice:

- persistent state is **binary-ish**: `Running` vs `Needs input`
- transient notification text preserves more nuance: `Permission`, `Waiting`, `Error`, `Attention`

Notification lifecycle:

- notification items are stored with `isRead`
- focusing the target surface marks the notification read
- tests also verify a flash is triggered on focus dismissal
- notifications are suppressed when app and panel focus are already active
- `clear_notifications` clears the queued notification list
- `prompt-submit` explicitly calls `clear_notifications`

Codex-specific evidence:

- `docs/notifications.md` documents a Codex integration via `~/.codex/config.toml notify = [...]`
- that integration extracts `last-assistant-message` from the completion payload and falls back to `Turn complete`
- no Codex-specific real-time status FSM was found in the inspected `cmux` sources

### Interpretation

For current `cmux`, Codex support is notification-oriented, not state-machine-oriented. The repo clearly supports Codex completion notifications, but the richer real-time status path in the shipped code is Claude-specific.

### Takeaway for agtmux

`cmux` is useful as evidence for two ideas:

- persistent status can be coarser than notification classification
- focus-aware clearing matters for notifications

It is **not** evidence that Codex itself should collapse `waiting_input` and `waiting_approval` into one internal status. That collapse is a `cmux` UI choice.

## 2. CodexMonitor

### Confirmed

`CodexMonitor` splits thread UI state and system notification policy.

#### Thread row / home status vocabulary

`src/utils/threadStatus.ts` defines:

- dot classes:
  - `processing`
  - `reviewing`
  - `unread`
  - `ready`
- home labels:
  - `Running`
  - `Reviewing`
  - `Idle`

Priority is intentionally asymmetric:

- pending user input wins over processing
- review wins over non-review idle
- processing wins over unread in the home-card view

The explicit test is important:

- `getThreadStatusClass(..., hasPendingUserInput=true)` returns `unread` even when `isProcessing=true`

#### What drives those states

`thread/status/changed` handling in `useThreadTurnEvents.ts`:

- `active` -> `markProcessing(true)`
- `idle`, `notLoaded`, `systemError` -> `markProcessing(false)`

Important nuance:

- the handler normalizes only `status.type`
- it does **not** inspect `activeFlags`
- so `waitingOnApproval` and `waitingOnUserInput` are not first-class dot colors here

Reviewing is handled elsewhere:

- `enteredReviewMode` item -> `markReviewing(true)`
- `exitedReviewMode` item -> `markReviewing(false)` and `markProcessing(false)`

User input attention is handled through request state:

- `item/tool/requestUserInput` becomes a tracked `userInputRequest`
- `ThreadRow.tsx` computes `hasPendingUserInput` from `pendingUserInputKeys`
- `getThreadStatusClass(...)` then maps that to `unread`

Approval attention is handled differently:

- request methods ending in `requestApproval` are collected
- they drive approval toasts and system notifications
- they do **not** appear to feed a dedicated thread-row dot state

#### Notification policy

`useAgentResponseRequiredNotifications.ts` sends immediate system notifications for:

- approval requests
- user input questions

Conditions:

- feature enabled
- window not focused
- 1.5s throttle window satisfied
- optional subagent muting not blocking the thread

Payload type:

- approval -> `extra.type = "approval"`
- question -> `extra.type = "question"`
- both are sent with `autoCancel: true`

`useAgentSystemNotifications.ts` handles completion/error notifications separately.

Success/error notifications are sent for:

- `turn/completed`
- `agentMessageCompleted`
- non-retrying turn errors

Conditions:

- feature enabled
- window not focused
- thread not muted as subagent
- duration is above `minDurationMs` (default `60000`)
- per-thread 1.5s re-notify guard

This means `CodexMonitor` does **not** treat completion as the same class of attention as approval/input:

- approvals/questions -> immediate attention
- task completion -> delayed success/error notification

#### Clearing / dedupe

The notification lifecycle is key-based, not state-name-based.

- approval dedupe key: `workspaceId:requestId`
- question dedupe key: `workspaceId:requestId`
- when an approval or question disappears from the active list, its key is removed
- if the same request id later reappears after resolution, it can notify again
- system notifications are not tracked with read/unread state in the app; they rely on `autoCancel` and OS behavior

### Interpretation

`CodexMonitor` is the clearest example of a good separation:

- one layer for in-app status dots
- one layer for immediate response-required notifications
- one layer for slower completion/error notifications

It also shows a useful anti-pattern:

- the app-server protocol exposes `waitingOnApproval` and `waitingOnUserInput`
- but the thread-row dot currently collapses those richer flags into simpler local classes

### Takeaway for agtmux

This is the best nearby precedent for notification policy:

- `waiting_approval` -> immediate attention
- `waiting_input` -> immediate attention
- `task_complete` / `turn completed` -> separate success policy, probably rate-limited / background-only

## 3. OpenAI Codex

### Confirmed: protocol-level status types

OpenAI's `codex` exposes two different status vocabularies.

#### Thread status

`ThreadStatusChangedNotification.json` and `thread_status.rs` define:

- `notLoaded`
- `idle`
- `systemError`
- `active`
  - optional flags:
    - `waitingOnApproval`
    - `waitingOnUserInput`

`thread_status.rs` computes this from runtime facts:

- `note_turn_started(...)`
  - marks thread loaded and running
  - result: `active` with no flags
- `note_permission_requested(...)`
  - increments pending permission counter
  - result: `active` with `waitingOnApproval`
- `note_user_input_requested(...)`
  - increments pending input counter
  - result: `active` with `waitingOnUserInput`
- both counters set
  - result: `active` with both flags
- `note_turn_completed(...)` or `note_turn_interrupted(...)`
  - clears running and both pending counters
  - result: `idle`
- `note_system_error(...)`
  - result: `systemError`
- `note_thread_shutdown(...)`
  - result: `notLoaded`

The tests explicitly cover:

- `active -> idle` after turn completion
- approval flag
- user input flag
- both flags together
- `thread/status/changed` notification emission

There is also a race-protection rule:

- if the server knows a turn is in progress, `resolve_thread_status(...)` upgrades `idle` / `notLoaded` to `active`

#### Agent status

`AgentStatus.ts` defines a separate agent lifecycle:

- `pending_init`
- `running`
- `completed`
- `errored`
- `shutdown`
- `not_found`

This is not the same thing as thread row status.

### Confirmed: review mode is separate from thread active flags

Review mode is represented by thread items such as:

- `enteredReviewMode`
- `exitedReviewMode`

The review test suite confirms these markers are emitted in review flows. In the open-source sources inspected here, review mode is **not** modeled as a `ThreadActiveFlag`.

### Confirmed: built-in user notification path is completion-oriented

`docs/config.md` says `notify` runs when the agent finishes a turn.

`hooks/src/user_notification.rs` shows the legacy notify payload shape:

- event type: `agent-turn-complete`
- includes:
  - `thread-id`
  - `turn-id`
  - `cwd`
  - `client`
  - `input-messages`
  - `last-assistant-message`

This confirms the shipped hook-based notification path is for completion, not waiting-state attention.

### Confirmed: TUI desktop notifications are one-shot

The TUI notification backend selects either:

- OSC 9
- BEL

based on terminal environment heuristics.

These are fire-and-forget terminal notifications. The inspected sources do not show:

- an in-app notification queue
- read/unread tracking
- explicit dismissal logic for waiting states

### Confirmed: waiting states have in-app UI, not proven desktop notifications

The open-source TUI contains in-app attention surfaces for pending interaction:

- `ApprovalOverlay`
- `PendingThreadApprovals`
- `PendingInputPreview`
- request-user-input overlay documented in `docs/tui-request-user-input.md`

Those are strong evidence that approval/user-input are treated as UI states. They are **not** evidence of built-in OS notification emission for those states.

### Inference

The exact status-dot implementation of the desktop app invoked by `codex app` is not exposed in the open-source repository inspected here. What is open-source and directly verifiable is:

- the protocol it exposes
- the TUI's approval/input UI
- the completion-notify hook

So the safest statement is:

- the official OpenAI status vocabulary for threads is `active(activeFlags)` / `idle` / `systemError` / `notLoaded`
- the official open-source evidence for desktop notifications is completion-oriented
- a desktop app status dot likely derives from `thread/status/changed`, but that rendering is an inference unless the app UI source is published separately

### Takeaway for agtmux

OpenAI `codex` provides the cleanest underlying model for `agtmux`:

- keep `running` separate from `waiting_*`
- keep `waiting_approval` and `waiting_input` as flags of an active session, not as a terminal `idle`
- clear waiting flags on request resolution and on turn completion/interruption
- do not assume completion notification equals response-required attention

## Cross-System Findings

### 1. Status and notification are separate concerns everywhere

All three systems separate persistent state from user notification policy.

- `cmux`: sidebar status vs notification panel
- `CodexMonitor`: thread row dot vs system notifications
- OpenAI `codex`: thread status vs completion notify hook

`agtmux` should keep doing this, but with sharper policy boundaries.

### 2. Waiting states deserve first-class modeling

The strongest common pattern is:

- waiting for user input is not plain idle
- waiting for approval is not plain idle

OpenAI `codex` makes this most explicit via `waitingOnApproval` and `waitingOnUserInput`.

### 3. Completion should not be treated as "attention"

Only `CodexMonitor` cleanly separates these today:

- response required -> immediate attention
- completion -> optional success notification

That split matches actual user value better than `task_complete => attention`.

### 4. Clearing should be event-driven, not timer-driven

The best clearing models seen here are:

- focus/read-driven for queue-style notifications (`cmux`)
- request-key lifecycle-driven for approval/input notifications (`CodexMonitor`)
- guard / counter resolution-driven for status flags (OpenAI `codex`)

This is stronger than "emit waiting_input, then later overwrite with idle heartbeat".

## Recommendations for agtmux

### Recommended status model

Keep the current explicit activity vocabulary:

- `running`
- `waiting_input`
- `waiting_approval`
- `idle`
- `error`

Do not collapse `waiting_input` into `idle`.

### Recommended notification model

Split notifications into two policies.

Policy A: response-required attention

- trigger immediately on `waiting_input`
- trigger immediately on `waiting_approval`
- suppress when focused on the relevant pane/session
- clear on consume events:
  - user prompt submit
  - approval response
  - pane/session focus if you choose a queue-style UI

Policy B: completion / success

- trigger only on real completion
- probably background-only
- probably duration-gated or rate-limited
- never use this to stand in for waiting-state attention

### Recommended clearing semantics

Track attention by request/session identity, not just by latest activity label.

Suggested model:

- `waiting_input` attention key:
  - provider + session_key + turn_id or event_seq
- `waiting_approval` attention key:
  - provider + session_key + approval request id
- clear the key when the underlying request disappears or is answered
- allow a reused id to notify again only after it fully leaves active state

### Recommended research-derived UX rule

If you want one compact pane badge, copy `cmux`'s UI simplification only at the display layer:

- internal state stays distinct
- badge text may collapse to `Needs input`
- notification subtitle/body preserves whether it is approval vs plain input

That preserves correctness without overcomplicating the UI.

## Bottom Line

If `agtmux` wants behavior closest to the strongest parts of these systems, the best hybrid is:

- OpenAI `codex` for the status ontology
- CodexMonitor for notification policy
- cmux for focus-aware queue clearing and lightweight badge text

That leads to:

- `running`
- `waiting_input`
- `waiting_approval`
- `idle`
- `error`

with notifications split into:

- immediate attention for `waiting_*`
- optional success/error for completed turns

and with clearing based on focus or request resolution, not a generic `idle`.
