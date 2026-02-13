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
- NFR-9: Sensitive connection data must not be stored in plaintext state DB.

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
     - then `event_time`
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
  - `pane_epoch`
  - `agent_type`
  - `pid` (nullable)
  - `started_at`
  - `ended_at` (nullable)
  - UNIQUE: (`target_id`, `pane_id`, `pane_epoch`)
- `events`
  - `event_id` (PK)
  - `runtime_id`
  - `event_type`
  - `source` (`hook`/`notify`/`wrapper`/`poller`)
  - `source_event_id` (nullable)
  - `source_seq` (nullable)
  - `event_time`
  - `ingested_at`
  - `dedupe_key`
  - `raw_payload`
  - UNIQUE: (`runtime_id`, `source`, `dedupe_key`)
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
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `state_version`
  - `observed_at`
  - `expires_at`
  - `nonce`
- `adapters`
  - `adapter_name` (PK)
  - `agent_type`
  - `version`
  - `capabilities` (JSON)
  - `enabled`
  - `updated_at`

### 7.4 Signal Ingestion

Common event envelope fields:
- `runtime_id`
- `source`
- `source_event_id` or `dedupe_key`
- `source_seq` (if available)
- `event_time`
- `ingested_at`

Runtime binding rule:
- If adapter event lacks `runtime_id`, ingest as `pending_bind` with `target_id + pane_id (+ pid/start_hint if present)`.
- Resolver binds pending event to current runtime (`bound`) or drops as `dropped_unbound` after TTL.
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
- `agtmux list panes [--target <name>|--all-targets] [--target-session <target>/<session>] [--session <name>] [--state <state>] [--agent <type>] [--needs-action] [--json]`
- `agtmux list windows [--target <name>|--all-targets] [--target-session <target>/<session>] [--session <name>] [--with-agent-status] [--json]`
- `agtmux list sessions [--target <name>|--all-targets] [--agent-summary] [--group-by target-session|session-name] [--json]`
- `agtmux attach <ref> [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux send <ref> --text <text> [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux view-output <ref> [--lines <n>]`
- `agtmux kill <ref> [--signal INT|TERM|KILL] [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale] [--yes]`
- `agtmux watch [--target <name>|--all-targets] [--scope panes|windows|sessions] [--format table|jsonl] [--interval <duration>] [--since <timestamp>] [--once]`

Canonical action reference grammar:
- `runtime:<runtime_id>`
- `pane:<target>/<session>/<window>/<pane>`

Reference resolution:
1. no match -> `E_REF_NOT_FOUND`
2. multiple match -> `E_REF_AMBIGUOUS` (must not execute action)
3. single match -> action snapshot is created server-side before execution

Output expectations:
- Default: human-readable table.
- `--json`: stable schema for automation and future UI.
- `watch --format jsonl` must emit one event per line with stable schema.
- JSON must include `schema_version`, `generated_at`, `filters`, `summary`, and per-item `identity`.
- Identity fields by scope:
  - `panes`: `target`, `session_name`, `window_id`, `pane_id`
  - `windows`: `target`, `session_name`, `window_id`
  - `sessions`: `target`, `session_name`

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
- Before any action (`attach`, `send`, `view-output`, `kill`), daemon must create `action_snapshot` containing `target`, `pane`, `runtime_id`, `state_version`, `observed_at`, `expires_at`.
- Action execution must fail closed if current state no longer matches snapshot and `--force-stale` is not explicitly set.

- `send`:
  - sends keystrokes/text to target pane via tmux on target.
  - must fail closed if runtime/state/freshness guard does not match.
- `attach`:
  - jumps user to pane/session on target safely.
  - must validate freshness/runtime before attach unless `--force-stale`.
- `view-output`:
  - uses pane capture, bounded by line limit.
- `kill`:
  - default `INT` for graceful interruption.
  - `TERM`/`KILL` only by explicit option.
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

## 8. High-Level Plan / Phases

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

Exit criteria:
- manual polling no longer required for Claude/Codex workflows across host and VM targets.
- listing and watch remain usable with partial target failures.
- visibility latency target is met (`p95 <= 2s` on supported environments).

### Phase 1.5: Control MVP

- Implement `send`, `view-output`, `kill` with fail-closed precondition checks.
- Add audit trail for control actions via correlated events.

Exit criteria:
- control actions are safe against stale pane/runtime mapping.
- stale action attempts are rejected by snapshot guard in integration tests.

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
5. Gemini strategy for MVP is interactive CLI first, with scripted ingestion as a supported extension path.
6. Architecture must stay adapter-first so Copilot CLI and Cursor CLI can be added incrementally.
7. Default session grouping is `target-session`; cross-target session-name merge is explicit.
8. Mutating actions are fail-closed by default via server-side action snapshot validation.
9. Action refs must be unambiguous (`runtime:` or fully-qualified `pane:`).

Open questions:
1. When Gemini provides stronger native event hooks, should it become the default ingestion path over parser-based fallback?
2. Should aggregated default view remain `--all-targets`, or switch to current-target default in very large environments?
