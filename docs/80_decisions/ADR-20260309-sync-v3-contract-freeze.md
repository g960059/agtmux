# ADR 20260309: sync-v3 Contract Freeze

## Status
- Accepted

## Context
- `ActivityState` is too lossy for the v3 rollout. It collapses completion into waiting, review into approval, and tool execution into generic running.
- The rollout plan requires fixture-first artifact freeze in the daemon repo before term migration.
- `agtmux-term` depends on strict pane/session identity and must not redefine the daemon contract independently.

## Decision
- Phase 0 freeze lives in this repo.
- `agtmux` daemon is the canonical producer of sync-v3 semantic truth.
- Downstream consumers are expected to treat sync-v3 as product truth and must not fall back to sync-v2 / collapsed legacy status when sync-v3 is missing or incompatible.
- `provider_raw` remains additive debug detail only and must not be used to reconstruct canonical semantic truth in consumers.
- The frozen bootstrap contract for this slice is:

```json
{
  "version": 3,
  "generated_at": "2026-03-09T20:11:04Z",
  "panes": [
    {
      "session_name": "workbench",
      "window_id": "@5",
      "session_key": "codex:%12",
      "pane_id": "%12",
      "pane_instance_id": {
        "pane_id": "%12",
        "generation": 7,
        "birth_ts": "2026-03-09T20:09:54Z"
      },
      "provider": "codex",
      "presence": "managed",
      "agent": { "lifecycle": "running" },
      "thread": {
        "lifecycle": "active",
        "blocking": "none",
        "execution": "thinking",
        "flags": {
          "review_mode": false,
          "subagent_active": false
        },
        "turn": {
          "outcome": "none",
          "sequence": 42,
          "started_at": "2026-03-09T20:10:00Z",
          "completed_at": null
        }
      },
      "pending_requests": [],
      "attention": {
        "active_kinds": [],
        "highest_priority": "none",
        "unresolved_count": 0,
        "generation": 0,
        "latest_at": null
      },
      "freshness": {
        "snapshot": "fresh",
        "blocking": "fresh",
        "execution": "fresh"
      },
      "provider_raw": {
        "codex": {
          "thread_status_type": "active",
          "active_flags": [],
          "agent_status": "running"
        }
      },
      "updated_at": "2026-03-09T20:11:04Z"
    }
  ]
}
```

- Exact identity is strict and required.
  - Required non-null fields: `session_name`, `window_id`, `session_key`, `pane_id`, `pane_instance_id`.
  - `pane_instance_id.pane_id` must match top-level `pane_id`.
  - `pane_id` alone is not a unique row key in linked-session topologies. If tmux inventory exposes the same live pane through multiple exact locations, v3 bootstrap/changes may emit multiple rows sharing `pane_id` while differing in `session_name` and/or `window_id`.
  - When a plain shell row is promoted to managed truth at the same visible location, `pane_instance_id` should stay stable while `session_key` may legitimately change from `shell:%pane_id` to `<provider>:%pane_id`.
  - In `ui.changes.v3`, an exact-identity change at the same visible location must be represented as `remove(old exact identity)` followed by `upsert(new exact identity)`, not as an in-place conflicting upsert.
  - If exact identity cannot be produced, the daemon must drop the pane from v3 output rather than emit partial/null identity.
- `pending_requests[].request_id` is the truth source for request identity.
- `attention` is summary only, never truth.
- `agent.lifecycle = completed` and `thread.lifecycle = idle` may coexist without being collapsed back into waiting state.
- Phase 0 fixtures are daemon-owned and are the decode source-of-truth for consumers.

### Frozen Enum Values

| Field | Values |
|---|---|
| `presence` | `managed`, `unmanaged`, `missing` |
| `agent.lifecycle` | `unknown`, `pending_init`, `running`, `completed`, `errored`, `shutdown`, `not_found` |
| `thread.lifecycle` | `not_loaded`, `active`, `idle`, `interrupted`, `errored`, `shutdown` |
| `thread.blocking` | `none`, `waiting_user_input`, `waiting_approval` |
| `thread.execution` | `none`, `thinking`, `streaming`, `tool_running`, `compacting` |
| `thread.turn.outcome` | `none`, `completed`, `aborted`, `errored` |
| `pending_requests[].kind` | `approval`, `user_input` |
| `pending_requests[].status` | `pending`, `resolved`, `dismissed` |
| `attention.active_kinds[]` | `question`, `approval`, `error`, `completion` |
| `attention.highest_priority` | `none`, `question`, `approval`, `error`, `completion` |
| `freshness.snapshot/blocking/execution` | `fresh`, `stale`, `down` |

### `pending_requests[]` Contract

- Active bootstrap snapshots expose only unresolved requests, so emitted entries must have `status = pending`.
- `resolved` and `dismissed` remain valid canonical statuses for daemon-internal tables and future change feeds.
- `thread.blocking = waiting_approval` requires at least one pending `approval` request.
- `thread.blocking = waiting_user_input` requires at least one pending `user_input` request.
- `thread.blocking = none` must not be inferred from `turn.outcome = completed`; explicit requests are authoritative.

### `attention` Semantics

- Truth inputs:
  - request truth: `pending_requests[]`
  - error truth: `agent.lifecycle`, `thread.lifecycle`, `thread.turn.outcome`
  - completion truth: `thread.turn.outcome`
- `attention.active_kinds[]` is emitted in descending daemon priority:
  - `error`
  - `approval`
  - `question`
  - `completion`
- `attention.highest_priority` is derived from `active_kinds[]`.
- `attention.unresolved_count` counts pending requests only. It does not increment for completion or error summaries.
- `attention.latest_at` is the newest relevant timestamp among:
  - pending request `updated_at`
  - `thread.turn.completed_at` when completion/error attention is active
  - snapshot `updated_at` when error attention is active without a newer turn timestamp
- `attention.generation` is a summary counter only. It is never a request identity mechanism.

### `freshness` Semantics

- `blocking` represents confidence in `pending_requests[]` and `thread.blocking`.
- `execution` represents confidence in `thread.execution`.
- `snapshot` is the overall confidence floor for the row.
  - In this slice, `snapshot` must be at least as degraded as the worst of `blocking` and `execution`.
  - Producers may degrade `snapshot` further if broader row truth is stale/down.
- Freshness badges must not replace the semantic truth axes.

### `provider_raw` Envelope

- `provider_raw` is always an object.
- Frozen provider slots for this slice:
  - `codex`
  - `claude`
  - `gemini`
  - `copilot`
- Slot values are provider-specific JSON objects when present.
- Consumers must treat nested provider payloads as opaque/debuggable data.
- Unknown nested fields inside a provider slot are allowed and should be ignored by default renderers.

### Unknown Enum / Missing Field Handling

- Producers must never emit unknown canonical enum strings.
  - Unknown provider-native values must be normalized into the frozen canonical fallback values such as `unknown`, `none`, `not_loaded`, or `down`.
  - Original provider-native detail belongs in `provider_raw`.
- Missing required identity fields are contract violations.
  - Producer action: drop the row or fail the payload build.
  - Consumer action: strict consumers may reject the row/payload.
- Unknown canonical enum values are protocol drift.
  - Strict consumers may fail closed.
  - Additive object fields remain allowed and should be ignored when unrecognized.

### Frozen Fixtures

- Source of truth path: `fixtures/sync-v3/`
- Required scenario set:
  - `codex-running.json`
  - `codex-waiting-approval.json`
  - `codex-completed-idle.json`
  - `claude-approval.json`
  - `claude-stop-idle.json`
  - `unmanaged-demotion.json`
  - `error.json`
  - `freshness-degraded.json`

## Consequences
- Positive:
  - The daemon repo now owns the v3 contract artifacts instead of leaving them implicit in term-side decoders.
  - Fixture-first validation can detect enum drift and identity regressions before live wire work is complete.
- Negative / risks:
  - `ui.bootstrap.v3` live output is not fully wired yet, so fixtures are ahead of live production state.
  - Some current source mappings still encode v2-era semantics and will need Phase 2 normalization work to match this freeze exactly.

## Alternatives
- A: Keep the contract informal in design prose only.
  - Rejected because cross-repo migration would drift immediately.
- B: Reuse `ActivityState` as the bootstrap truth and add a few extra fields later.
  - Rejected because it preserves the exact semantic errors this rollout is intended to remove.
- C: Wait to freeze artifacts until `ui.changes.v3` is ready.
  - Rejected because `ui.bootstrap.v3` must land first and consumers need fixtures now.

## Links
- Design input:
  - `/tmp/agtmux-status-v3-final-design-20260309.md`
- Research:
  - `docs/research/20260309-status-notification-comparison.md`
  - `docs/research/claude-jsonl-waiting-states.md`
- Fixtures:
  - `fixtures/sync-v3/README.md`
