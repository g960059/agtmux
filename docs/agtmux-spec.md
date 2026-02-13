# AGTMUX Spec (v0.5)

Date: 2026-02-13
Status: Draft

## 1. Background

Current workflows rely on tmux for organizing work by session/window/pane, while agent CLIs (Claude, Codex, Gemini) run in panes.
The current pain is that operator awareness depends on manual polling: checking panes repeatedly to know whether agents are running, waiting for user input, completed, or idle.

The desired direction is:
- tcmux-like visibility for tmux + agent state
- multi-agent support (Claude, Codex, Gemini)
- adapter-based extensibility for future agents (for example Copilot CLI, Cursor CLI)
- grouping and summaries by tmux session/window
- unified view across host and VMs (host, vm1, vm2)
- eventual macOS resident app for fast control actions

## 2. Problems

- No always-on unified status view across panes.
- Manual polling is expensive and error-prone.
- Agent state semantics differ per CLI and are not normalized.
- tmux structures can get noisy as sessions/windows/panes grow.
- There is no single control surface for `send`, `attach`, `view output`, and `kill`.
- Host/VM contexts are separated operationally, reducing global awareness.

## 3. Goals

- Provide always-available state visibility for agent panes without manual polling.
- Support Claude, Codex, and Gemini with a unified state model.
- Provide tcmux-like listing commands:
  - list agent panes
  - list windows (including agent status summary)
  - list agent sessions
- Unify host and VM targets in one view while preserving target identity.
- Categorize by status (running, waiting_input, waiting_approval, completed, idle, error, unknown).
- Categorize cross-target by tmux session for quick project-level awareness.
- Default grouping must be `target-session` to avoid cross-target session-name collisions.
- Enable operational actions from one tooling surface:
  - send
  - attach
  - view output
  - kill
- Keep architecture reusable so the same backend powers both CLI and future macOS resident app.

## 4. Non-Goals (Initial)

- Replacing tmux itself.
- Building a full terminal emulator UI.
- Perfect semantic understanding of every possible third-party CLI output format.
- Cloud sync or multi-host distributed orchestration in v0.

## 5. User Stories

1. As a developer, I want to see all agent panes and their states at once across host and VMs, so I can stop checking each pane manually.
2. As a developer, I want a session-level summary grouped across targets, so I can quickly decide which project area needs attention.
3. As a developer, I want to filter by state (for example waiting_input/waiting_approval), so I can prioritize intervention.
4. As a developer, I want to attach to the exact pane where an agent is waiting.
5. As a developer, I want to send input to a pane from CLI/app without switching context.
6. As a developer, I want to inspect recent output and then kill or resume the agent safely.

## 6. Requirements

### 6.1 Functional Requirements

- FR-1: Detect and persist agent states for Claude and Codex in MVP, and Gemini by Phase 2.
- FR-2: Normalize agent-specific signals into a common state model.
- FR-3: Maintain mapping between tmux pane and agent runtime metadata.
- FR-4: Provide pane/window/session listing with state summaries.
- FR-5: Support control commands: send, attach, view-output, kill.
- FR-6: Offer machine-readable output (`--json`) for all list commands.
- FR-7: Include session/window grouping and counts by state.
- FR-8: Include a reconciliation mechanism so stale states self-heal.
- FR-9: Support multiple targets (host/vm1/vm2) in one aggregated view.
- FR-10: Provide target commands (`add`, `connect`, `list`, `remove`) and target-scoped operations.
- FR-11: Provide adapter registry so new agent CLIs can be added without changing core state engine.
- FR-12: Support future adapters such as Copilot CLI and Cursor CLI.
- FR-13: Introduce runtime identity guard (`runtime_id`/`pane_epoch`) to prevent stale action/event application.
- FR-14: Define deterministic event dedupe and ordering semantics.
- FR-15: All actions must enforce fail-closed preconditions via server-side snapshot validation.
- FR-16: Define canonical action reference grammar and unambiguous resolution rules.

### 6.2 Non-Functional Requirements

- NFR-1: Near real-time updates (target: <= 2 seconds visible lag).
- NFR-2: Safe failure mode: unknown state instead of incorrect definitive state.
- NFR-3: Low overhead on host CPU/memory.
- NFR-4: Idempotent event handling and robust against duplicate events.
- NFR-5: Extensible adapter model for future agents.
- NFR-6: Adapter contract stability (backward-compatible interface across minor versions).
- NFR-7: Partial-result operation under target failures (do not block global listings).
- NFR-8: Deterministic convergence for same input event stream.
- NFR-9: Sensitive connection data and unredacted payloads must not be stored in plaintext state DB.

## 7. Specification

### 7.1 Core Architecture

- Target Manager:
  - stores known targets (`host`, `vm1`, `vm2`, ...).
  - manages connectivity and remote command execution context.
- Target Executor:
  - unified local/ssh execution abstraction for tmux operations and adapter runtime.
  - required for all read/write operations to targets.
- Collector (per target):
  - collects tmux topology and adapter signals from one target.
- Aggregator / Daemon (`agtmuxd`):
  - merges events from collectors.
  - owns state engine and persistence.
  - serves read/write API for CLI and future macOS app.
- Agent Adapter Registry:
  - maps `agent_type` to adapter implementation and capabilities.
  - core engine only depends on adapter interface, not concrete agent logic.
- Agent Adapters (initial):
  - Claude adapter: hook-driven events.
  - Codex adapter: notify-driven events plus wrapper lifecycle signals.
  - Gemini adapter: wrapper lifecycle + output parser signals.
  - future adapters: Copilot CLI, Cursor CLI (no core redesign required).
- Tmux Observer:
  - tracks pane/session/window topology per target via tmux metadata.
- State Engine:
  - merges adapter events and tmux observations into canonical state.
- State Store:
  - durable store (SQLite recommended) as the single source of truth.
- Presentation Layer:
  - CLI commands now.
  - macOS resident app later, reading the same daemon API.

### 7.2 Canonical State Model

Canonical states:
- `running`
- `waiting_input`
- `waiting_approval`
- `completed`
- `idle`
- `error`
- `unknown`

State precedence (highest first):
1. `error`
2. `waiting_approval`
3. `waiting_input`
4. `running`
5. `completed`
6. `idle`
7. `unknown`

Defaults (recommended):
- `completed` auto-demotes to `idle` after 120 seconds (configurable).
- `kill` default signal is `INT`.

Notes:
- `completed` means last task ended and is still fresh for operator recognition.
- Resolution order is `dedupe/order check -> runtime guard -> freshness check -> precedence`.
- Unknown or stale information must not be promoted to a confident active state.

### 7.2.1 Transition and Safety Rules

- Every event must include enough metadata to dedupe and order safely.
- Event application must be dropped if runtime identity does not match current pane runtime.
- Precedence is applied only among fresh candidate signals.
- Demotion job (`completed -> idle`) must include a version/runtime guard.
- Demotion time base is daemon `ingested_at` (not remote wall clock).
- `unknown` must include `reason_code` (for example `stale_signal`, `target_unreachable`, `unsupported_signal`).
- If target health is `down` or signals are stale beyond TTL, state resolution must short-circuit to `unknown`.

### 7.2.2 Ordering and Dedupe Algorithm

- Event apply order is deterministic:
  1. reject duplicates by `(runtime_id, source, dedupe_key)`
  2. compare ordering key:
     - `source_seq` when available (same `runtime_id + source`)
     - else `effective_event_time` where
       - `effective_event_time = event_time` when `abs(event_time - ingested_at) <= skew_budget`
       - `effective_event_time = ingested_at` when skew is larger than budget
       - default `skew_budget` is 10 seconds (configurable)
     - then `ingested_at`
     - then `event_id`
  3. apply only if key is newer than stored cursor for that `runtime_id + source`
- A source-specific cursor must be maintained to avoid cross-source starvation.

### 7.2.3 Adapter Contract

Each adapter MUST implement a common contract:
- `ContractVersion() -> string`
- `IdentifyProcess(ctx, pane) -> match_result`
- `Subscribe(ctx, pane) -> signal_stream` (for event-driven adapters)
- `Poll(ctx, pane) -> []signal` (for polling fallback)
- `Normalize(signal) -> state_transition`
- `Health(ctx) -> status`

Capabilities are declared by adapter:
- `event_driven`
- `polling_required`
- `supports_waiting_approval`
- `supports_waiting_input`
- `supports_completed`

Core behavior:
- State Engine consumes only normalized transitions.
- Unknown or unsupported signals must degrade to `unknown`, never fabricated states.

### 7.2.4 Runtime Identity and Pane Epoch Rules

- Runtime identity is anchored to pane instance:
  - `pane_instance = (target_id, tmux_server_boot_id, pane_id)`
- `pane_epoch` MUST increment when any of the following happens:
  - pane is recreated with same `pane_id` after layout churn or restart
  - adapter/observer detects runtime process identity change (`pid`) for the pane
  - observer resync finds active runtime row that no longer matches current pane process identity
- `runtime_id` MUST be unique and reproducible from runtime metadata:
  - recommended derivation:
    - `sha256(target_id + tmux_server_boot_id + pane_id + pane_epoch + agent_type + started_at_ns)`
- At most one active runtime (`ended_at IS NULL`) is allowed per `(target_id, pane_id)`.
- Events or actions referencing stale runtime identity MUST be rejected (`E_RUNTIME_STALE` / precondition failure).

### 7.3 Data Model (SQLite draft)

- `targets`
  - `target_id` (PK)
  - `target_name` (`host`, `vm1`, `vm2`, ...)
  - `kind` (`local`/`ssh`)
  - `connection_ref` (non-secret reference such as ssh host alias)
  - `is_default`
  - `last_seen_at`
  - `health` (`ok`/`degraded`/`down`)
  - `updated_at`
- `panes`
  - `target_id`
  - `pane_id`
  - `session_name`
  - `window_id`
  - `window_name`
  - `updated_at`
  - PK: (`target_id`, `pane_id`)
- `runtimes`
  - `runtime_id` (PK)
  - `target_id`
  - `pane_id`
  - `tmux_server_boot_id`
  - `pane_epoch`
  - `agent_type`
  - `pid` (nullable)
  - `started_at`
  - `ended_at` (nullable)
  - UNIQUE: (`target_id`, `tmux_server_boot_id`, `pane_id`, `pane_epoch`)
  - Active runtime invariant:
    - at most one active runtime (`ended_at IS NULL`) per (`target_id`, `pane_id`)
    - enforce with partial unique index at DB level
- `events`
  - `event_id` (PK)
  - `runtime_id`
  - `event_type`
  - `source` (`hook`/`notify`/`wrapper`/`poller`)
  - `source_event_id` (nullable)
  - `source_seq` (nullable)
  - `event_time`
  - `ingested_at`
  - `dedupe_key` (NOT NULL)
  - `action_id` (nullable, FK -> `actions.action_id`)
  - `raw_payload` (redacted form; optional by policy)
  - UNIQUE: (`runtime_id`, `source`, `dedupe_key`)
- `event_inbox`
  - `inbox_id` (PK)
  - `target_id`
  - `pane_id`
  - `runtime_id` (nullable)
  - `event_type`
  - `source`
  - `dedupe_key`
  - `event_time`
  - `ingested_at`
  - `pid` (nullable)
  - `start_hint` (nullable)
  - `status` (`pending_bind`/`bound`/`dropped_unbound`)
  - `reason_code` (nullable)
  - `raw_payload` (redacted form; optional by policy)
  - UNIQUE: (`target_id`, `pane_id`, `source`, `dedupe_key`)
- `runtime_source_cursors`
  - `runtime_id`
  - `source`
  - `last_source_seq` (nullable)
  - `last_order_event_time`
  - `last_order_ingested_at`
  - `last_order_event_id`
  - PK: (`runtime_id`, `source`)
- `states`
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `state`
  - `reason_code`
  - `confidence` (`high`/`medium`/`low`)
  - `state_version`
  - `last_source_seq` (nullable)
  - `last_seen_at`
  - `updated_at`
  - PK: (`target_id`, `pane_id`)
- `action_snapshots`
  - `snapshot_id` (PK)
  - `action_id` (FK -> `actions.action_id`)
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `state_version`
  - `observed_at`
  - `expires_at`
  - `nonce`
- `actions`
  - `action_id` (PK)
  - `action_type` (`attach`/`send`/`view-output`/`kill`)
  - `request_ref` (required idempotency key; UUIDv7 or ULID recommended)
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `requested_at`
  - `completed_at` (nullable)
  - `result_code`
  - `error_code` (nullable)
  - `metadata_json`
  - UNIQUE: (`action_type`, `request_ref`)
- `adapters`
  - `adapter_name` (PK)
  - `agent_type`
  - `version`
  - `capabilities` (JSON)
  - `enabled`
  - `updated_at`

### 7.3.1 Data Protection, Retention, and Index Baseline

- `targets.connection_ref` MUST store only non-secret references (for example ssh host alias).
- `events.raw_payload` MUST be stored as redacted content by default.
- Unredacted payload MUST NOT be persisted in SQLite in any mode.
- Debug mode may keep unredacted payload only in memory or encrypted temporary file storage with max TTL 24 hours.
- Default retention policy:
  - `events` raw payload: 7 days
  - `events` metadata rows: 14 days (configurable)
- Required baseline indexes:
  - unique partial: `runtimes(target_id, pane_id) WHERE ended_at IS NULL`
  - unique: `actions(action_type, request_ref)`
  - `events(runtime_id, source, ingested_at DESC)`
  - `events(ingested_at DESC)`
  - `event_inbox(status, ingested_at)`
  - `states(updated_at DESC)`
  - `states(state, updated_at DESC)`

### 7.4 Signal Ingestion

### 7.4.1 Event Envelope v1 (Normative)

Common event envelope fields:
- `event_id` (required, string/UUID)
- `event_type` (required, string)
- `source` (required, enum: `hook|notify|wrapper|poller`)
- `dedupe_key` (required, string, non-empty)
- `source_event_id` (optional, string)
- `source_seq` (optional, int64)
- `event_time` (required, timestamp from source)
- `ingested_at` (required, daemon timestamp)
- `runtime_id` (required for bound events, optional for pending-bind events)
- `target_id` (required when `runtime_id` is absent)
- `pane_id` (required when `runtime_id` is absent)
- `pid` (optional, pending-bind hint)
- `start_hint` (optional, pending-bind hint timestamp)
- `raw_payload` (optional; redacted form under default policy)

Envelope rules:
- `dedupe_key` MUST always be present, even when `source_event_id` exists.
- Recommended `dedupe_key` derivation:
  - `sha256(source + ":" + coalesce(source_event_id,"") + ":" + normalize(payload_hash) + ":" + normalize(event_type))`
- If `source_seq` is absent, ordering relies on 7.2.2 `effective_event_time`.
- `ingested_at` is authoritative for freshness and demotion timing decisions.

Runtime binding rule:
- If adapter event lacks `runtime_id`, ingest as `pending_bind` with `target_id + pane_id (+ pid/start_hint if present)`.
- Resolver MUST bind only when there is exactly one active runtime candidate that satisfies all available hints.
  - candidate set is filtered by `target_id + pane_id`
  - if `pid` exists, candidate runtime `pid` MUST match
  - if `start_hint` exists, `abs(runtime.started_at - start_hint)` MUST be within bind window (default 5 seconds)
- Resolver MUST drop pending event as `dropped_unbound` when:
  - no candidate exists (`reason_code = bind_no_candidate`)
  - more than one candidate exists (`reason_code = bind_ambiguous`)
  - pending-bind TTL expires before safe resolution (`reason_code = bind_ttl_expired`)
- Only `bound` events are eligible for state transitions.

Adapter-specific ingestion:
- Claude:
  - Use hooks for lifecycle and interaction-needed signals.
- Codex:
  - Use `notify` events (`approval-requested`, `agent-turn-complete`) + wrapper start/exit signals.
- Gemini:
  - Use wrapper start/exit signals + configurable parser patterns.
- Copilot CLI (future):
  - Adapter starts with wrapper + parser approach, then shifts to native events if exposed.
- Cursor CLI (future):
  - Adapter starts with wrapper + parser approach, then shifts to native events if exposed.

Reconciler:
- periodic tmux scan (default 2 seconds for active panes).
- exponential backoff for idle panes.
- stale demotion and target health transitions are reconciler-owned.
- target unreachable transitions must emit `unknown/target_unreachable` with low confidence.

### 7.5 CLI Surface (MVP)

Target management:
- `agtmux target add <name> --kind local|ssh [--ssh-target <ssh_host>]`
- `agtmux target connect <name>`
- `agtmux target list [--json]`
- `agtmux target remove <name> [--yes]`

Listings and actions (aggregated by default):
- `agtmux list panes [--target <name>|--all-targets] [--target-session <target>/<session-enc>] [--session <name>] [--state <state>] [--agent <type>] [--needs-action] [--json]`
- `agtmux list windows [--target <name>|--all-targets] [--target-session <target>/<session-enc>] [--session <name>] [--with-agent-status] [--json]`
- `agtmux list sessions [--target <name>|--all-targets] [--agent-summary] [--group-by target-session|session-name] [--json]`
- `agtmux attach <ref> [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux send <ref> (--text <text>|--stdin|--key <key>) [--enter] [--paste] [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux view-output <ref> [--lines <n>]`
- `agtmux kill <ref> [--mode key|signal] [--signal INT|TERM|KILL] [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale] [--yes]`
- `agtmux watch [--target <name>|--all-targets] [--scope panes|windows|sessions] [--format table|jsonl] [--interval <duration>] [--cursor <stream_id:sequence>] [--once]`

Canonical action reference grammar (BNF):

```txt
<ref> ::= <runtime-ref> | <pane-ref>
<runtime-ref> ::= "runtime:" <runtime-id>
<runtime-id> ::= /[A-Za-z0-9._:-]{16,128}/
<pane-ref> ::= "pane:" <target> "/" <session-enc> "/" <window-id> "/" <pane-id>
<target> ::= /[A-Za-z0-9._-]+/
<session-enc> ::= RFC3986 percent-encoded session name
<window-id> ::= "@" <digits>
<pane-id> ::= "%" <digits>
```

Reference component rules:
- `target` is the registered target name.
- `session-enc` MUST be percent-encoded before parsing.
- `window-id` / `pane-id` MUST use tmux immutable IDs, not display names.
- `--target-session` MUST use `<target>/<session-enc>` with the same encoding rule.

Reference resolution:
1. parse failure -> `E_REF_INVALID`
2. decode failure -> `E_REF_INVALID_ENCODING`
3. no match -> `E_REF_NOT_FOUND`
4. multiple match -> `E_REF_AMBIGUOUS` (must not execute action)
5. single match -> action snapshot is created server-side before execution

Output expectations:
- Default: human-readable table.
- `--json`: stable schema for automation and future UI.
- `watch --format jsonl` must emit one event per line with stable schema.
- JSON must include `schema_version`, `generated_at`, `filters`, `summary`, and per-item `identity`.
- If any target fails during aggregated read, JSON MUST include:
  - `partial` (boolean)
  - `requested_targets`
  - `responded_targets`
  - `target_errors` (per-target error list)
- Identity fields by scope:
  - `panes`: `target`, `session_name`, `window_id`, `pane_id`
  - `windows`: `target`, `session_name`, `window_id`
  - `sessions`: `target`, `session_name`

### 7.5.1 Watch JSONL Contract

- `watch --format jsonl` emits UTF-8 JSON lines, one event per line.
- Line envelope fields:
  - `schema_version`
  - `generated_at`
  - `emitted_at`
  - `stream_id`
  - `cursor` (`<stream_id>:<sequence>`)
  - `scope` (`panes|windows|sessions`)
  - `type` (`snapshot|delta|reset`)
  - `sequence` (monotonic within same `stream_id`)
  - `filters`
  - `summary`
  - `items` (for `snapshot`)
  - `changes` (for `delta`)
- `items[].identity` MUST follow scope-specific identity requirements from 7.5.
- `delta` lines MUST include per-item operation (`upsert|delete`) with identity.
- `cursor` resumes from the first event whose sequence is greater than requested cursor in the same stream.
- If provided cursor is invalid: `E_CURSOR_INVALID`.
- If cursor is expired (outside retention): emit `reset` then next `snapshot`, and return `E_CURSOR_EXPIRED` in API response mode.

### 7.6 Grouping and Summaries

- Global state rollup (all targets by default):
  - counts by state
  - counts by agent type
  - counts by target
- Session-level rollup:
  - default grouping: `target-session`
  - cross-target merge by `session-name` is explicit via `--group-by session-name`
  - each row includes per-target breakdown and total counts
- Window-level rollup:
  - top state by precedence
  - waiting counts
  - running counts

### 7.7 Control Behavior

Server-side action snapshot:
- Before any action (`attach`, `send`, `view-output`, `kill`), daemon MUST create `actions` row and `action_snapshot` containing `target`, `pane`, `runtime_id`, `state_version`, `observed_at`, `expires_at`, `nonce`.
- Action execution MUST fail closed if current state no longer matches snapshot and `--force-stale` is not explicitly set.
- CLI guard flags (`--if-runtime`, `--if-state`, `--if-updated-within`) are additional constraints only; they MUST NOT weaken server-side validation.
- Action execution MUST reject expired snapshot with `E_SNAPSHOT_EXPIRED`.
- Action write APIs MUST be idempotent by (`action_type`, `request_ref`):
  - same key replay returns existing `action_id` and stored result
  - conflicting replay with different payload returns `E_IDEMPOTENCY_CONFLICT`

- `send`:
  - sends keystrokes/text to target pane via tmux on target.
  - `--text` sends literal text (no shell interpolation).
  - `--stdin` sends stdin payload (for multiline input).
  - `--key` sends tmux key token (for example `C-c`, `Escape`).
  - `--enter` appends Enter after payload.
  - `--paste` uses paste-buffer style delivery for multiline safety.
  - must fail closed if runtime/state/freshness guard does not match.
- `attach`:
  - jumps user to pane/session on target safely.
  - must validate freshness/runtime before attach unless `--force-stale`.
- `view-output`:
  - uses pane capture, bounded by line limit.
- `kill`:
  - default mode is `key`; `INT` maps to `C-c` send-key behavior.
  - `--mode signal` sends OS signal to runtime `pid` on target.
  - `TERM`/`KILL` are valid only with `--mode signal`.
  - `--mode signal` MUST fail with `E_PID_UNAVAILABLE` when runtime `pid` is unknown.
  - confirmation prompt is required by default; `--yes` skips confirmation.
  - must fail closed on guard mismatch (`if-runtime`, `if-state`, freshness window).

### 7.8 macOS Resident App (Future)

- Reads the same daemon API as CLI.
- Primary screens:
  - Global summary (state + target)
  - Sessions summary (cross-target)
  - Windows in selected session
  - Panes detail list with state and last update age
- Actions from UI:
  - send
  - attach
  - view output
  - kill

### 7.9 agtmuxd API v1 (Normative Minimum)

Transport and versioning:
- Daemon API v1 is the shared backend contract for CLI and macOS app.
- Responses MUST include `schema_version`.
- Read responses MUST include `generated_at`.
- Action responses MUST include `action_id`, `result_code`, and `completed_at` (when completed).

Read endpoints (minimum):
- `GET /v1/panes`
- `GET /v1/windows`
- `GET /v1/sessions`
- `GET /v1/watch?scope=<panes|windows|sessions>&cursor=<stream_id:sequence>`

Write endpoints (minimum):
- `POST /v1/actions/attach`
- `POST /v1/actions/send`
- `POST /v1/actions/view-output`
- `POST /v1/actions/kill`

Action request minimum fields:
- `request_ref` (required idempotency key)
- `ref`
- `if_runtime` (optional)
- `if_state` (optional)
- `if_updated_within` (optional)
- `force_stale` (optional; default false)

Error code contract (minimum):
- `E_REF_INVALID`
- `E_REF_INVALID_ENCODING`
- `E_REF_NOT_FOUND`
- `E_REF_AMBIGUOUS`
- `E_RUNTIME_STALE`
- `E_PRECONDITION_FAILED`
- `E_SNAPSHOT_EXPIRED`
- `E_IDEMPOTENCY_CONFLICT`
- `E_CURSOR_INVALID`
- `E_CURSOR_EXPIRED`
- `E_PID_UNAVAILABLE`
- `E_TARGET_UNREACHABLE`

## 8. High-Level Plan / Phases

### 8.1 Delivery Artifacts (Recommended)

- Maintain a rolling implementation plan document (`plan`) per active phase.
- Maintain executable task breakdown (`task`) linked to FR/NFR IDs.
- Maintain a test catalog (`test`) that maps each critical contract to automated coverage.
- Gate phase completion on artifacts being updated together with code/spec changes.

### Phase 0: Core Runtime

- Define canonical state model, event envelope, and transition safety rules.
- Implement `TargetExecutor` and daemon boundary (`agtmuxd`).
- Implement SQLite store and minimal schema (including `runtimes`, dedupe fields, state version).
- Implement tmux topology observer per target.
- Implement reconciler and stale-state convergence rules.

Exit criteria:
- topology and state rows are persisted and queryable for multiple targets.
- stale states converge to safe values within configured TTL.
- deterministic ordering and dedupe behavior are validated by replay tests.

### Phase 1: Visibility MVP (Claude + Codex, Multi-Target)

- Implement target manager (`add/connect/list/remove`).
- Implement Claude hook adapter.
- Implement Codex notify + wrapper adapter.
- Implement list commands (panes/windows/sessions) with `target-session` default grouping.
- Implement `watch` and `attach`.
- Define and expose `agtmuxd API v1` read/watch contract.

Exit criteria:
- manual polling no longer required for Claude/Codex workflows across host and VM targets.
- listing and watch remain usable with partial target failures.
- visibility latency target is met (`p95 <= 2s` on supported environments).
- watch jsonl schema contract is validated by compatibility tests.
- attach fail-closed behavior is validated in stale-runtime integration tests.

### Phase 1.5: Control MVP

- Implement `send`, `view-output`, `kill` with fail-closed precondition checks.
- Add audit trail for control actions via correlated events.
- Expose API v1 write endpoints for control actions.

Exit criteria:
- control actions are safe against stale pane/runtime mapping.
- stale action attempts are rejected by snapshot guard in integration tests.
- action-to-event correlation is queryable by `action_id`.
- idempotent replay of action requests is validated by integration tests.

### Phase 2: Gemini + Reliability Hardening

- Add Gemini adapter.
- Add stronger reconnect handling and backoff tuning.
- Add richer filters/sorting and JSON schema hardening.

Exit criteria:
- all three agents supported with stable state convergence.

### Phase 2.5: Adapter Expansion

- Add Copilot CLI adapter (v1 capabilities).
- Add Cursor CLI adapter (v1 capabilities).
- Validate no core engine changes are required to onboard both.

Exit criteria:
- Copilot CLI and Cursor CLI states are visible in the same commands and summaries.
- adapter registry and capability flags drive behavior cleanly.

### Phase 3: macOS Resident App

- Build menu bar app over shared daemon API.
- Add actionable lists and fast operations.
- Keep CLI as first-class interface.

Exit criteria:
- app provides at-a-glance visibility and core control actions.

## 9. Risks and Mitigations

- Risk: parser fragility for terminal text patterns.
  - Mitigation: prioritize event-driven signals; parser as fallback only.
- Risk: race conditions from concurrent hooks/notifications.
  - Mitigation: transactional writes + dedupe key + sequence checks.
- Risk: stale state after crashes or disconnects.
  - Mitigation: reconciler with explicit health transitions and TTL-based demotion.
- Risk: session name collisions across targets.
  - Mitigation: default `target-session` grouping; `session-name` merge only on explicit request.
- Risk: stale runtime mapping causes wrong control target.
  - Mitigation: runtime guards and fail-closed control preconditions.
- Risk: sensitive connection or payload data leakage in local store.
  - Mitigation: store only non-secret `connection_ref`, apply payload redaction, and define retention TTL.
- Risk: over-coupling UI and collector internals.
  - Mitigation: enforce daemon API boundary.

## 10. Decisions and Open Questions

Decisions fixed:
1. State scope is unified across `host`, `vm1`, `vm2` (single aggregated view).
2. `completed -> idle` default demotion is 120 seconds (configurable).
3. `kill` default signal is `INT`.
4. Destructive actions (`kill`, `remove target`) require confirmation by default.
5. Gemini strategy is interactive CLI first when enabled in Phase 2, with scripted ingestion as a supported extension path.
6. Architecture must stay adapter-first so Copilot CLI and Cursor CLI can be added incrementally.
7. Default session grouping is `target-session`; cross-target session-name merge is explicit.
8. All actions are fail-closed by default via server-side action snapshot validation.
9. Action refs must be unambiguous (`runtime:` or fully-qualified `pane:`).

Open questions:
1. When Gemini provides stronger native event hooks, should it become the default ingestion path over parser-based fallback?
2. Should aggregated default view remain `--all-targets`, or switch to current-target default in very large environments?
