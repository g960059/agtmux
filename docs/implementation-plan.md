# AGTMUX Implementation Plan (v0.5)

Date: 2026-02-13
Status: Draft
Source Spec: `docs/agtmux-spec.md`

## 1. Purpose

This document defines the delivery plan for AGTMUX after the v0.5 spec update.
It sets phase goals, exit gates, dependency order, and change-control rules.

## 2. Contract Freeze Before Coding

The following contracts are treated as phase-entry prerequisites and must not change without explicit spec update:

- Action fail-closed behavior (`action_snapshot`, `runtime_id`, `state_version`, freshness checks).
- Action idempotency (`actions(action_type, request_ref)` uniqueness and replay behavior).
- Watch stream resume contract (`cursor=<stream_id:sequence>`).
- Runtime identity rules (`tmux_server_boot_id`, `pane_epoch`, stale runtime rejection).
- Event ordering and dedupe semantics (`source_seq`, `effective_event_time`, source cursor model).
- Daemon API transport contract (`HTTP/JSON over UDS`, health endpoint, lifecycle semantics).
- Target health transition policy and SSH execution timeouts/retry behavior.

## 3. Delivery Principles

- Ship in vertical slices: API + state engine + CLI for each capability.
- Block promotion when contract tests fail.
- Prioritize safety over completeness (`unknown` over incorrect definitive state).
- Keep CLI and daemon behavior aligned through shared API contracts.

## 4. Phase Roadmap

| Phase | Primary Outcome | Required Gates |
| --- | --- | --- |
| 0 | Core runtime and deterministic state engine | Ordering/dedupe replay, runtime guard, schema migration checks |
| 1 | Visibility MVP (Claude + Codex, multi-target read) | Watch cursor compatibility, partial-result behavior, attach safety |
| 1.5 | Control MVP | Snapshot failure tests, idempotent action replay, audit correlation |
| 2 | Gemini + reliability hardening | Drift/failure recovery tests, schema compatibility |
| 2.5 | Adapter expansion | Contract suite pass for each new adapter |
| 3 | macOS resident app | API v1 compatibility and action safety parity |

## 5. Phase Details

### Phase 0: Core Runtime

Objective:
- Build deterministic ingestion and canonical state persistence.

In scope:
- `TargetExecutor` implementation and daemon boundary (`agtmuxd`).
- Daemon transport and lifecycle baseline (`HTTP/JSON over UDS`, single-instance lock, graceful shutdown/restart behavior).
- tmux topology observer per target.
- SQLite schema + migrations for `runtimes`, `events`, `event_inbox`, `runtime_source_cursors`, `states`, `actions`, `action_snapshots`.
- Runtime identity + pane epoch lifecycle.
- Ordering/dedupe application path.
- Reconciler for stale/health transitions.

Entry criteria:
- Spec sections 7.2, 7.3, 7.4, 7.7 frozen.

Exit criteria:
- Topology and state rows are persisted and queryable for multiple targets.
- Stale states converge to safe values within configured TTL.
- Deterministic ordering and dedupe behavior validated by replay tests.
- Active runtime uniqueness constraint enforced at DB level.
- Security/performance baseline tests are green (`TC-011`, `TC-012`, `TC-013`).
- Target execution and topology observation tests are green (`TC-040`, `TC-041`).
- CI/Nightly execution baseline exists for tmux + ssh scenarios and stores replay artifacts.

### Phase 1: Visibility MVP

Objective:
- Replace manual polling for Claude/Codex across host + VM targets.

In scope:
- Target manager (`add/connect/list/remove`).
- Claude and Codex adapters.
- List commands (`panes/windows/sessions`) + watch stream.
- Structured error envelope baseline for API/CLI parity.
- Shared action write path/idempotency for attach flow (`actions`, `request_ref`).
- Attach with fail-closed snapshot validation using shared action infrastructure.
- API v1 read/watch endpoints.
- Minimal API write endpoint for attach path.

Entry criteria:
- Phase 0 gates passed.

Exit criteria:
- Manual polling no longer required for Claude/Codex workflows.
- Listing/watch usable under partial target failure.
- Visibility latency p95 <= 2s (defined benchmark profile).
- Watch jsonl schema compatibility tests pass.
- API/CLI error envelope baseline is stable for read/attach paths.
- Attach stale-runtime rejection tests pass.
- Grouping and multi-target aggregation semantics tests pass (`TC-042`, `TC-043`).

### Phase 1.5: Control MVP

Objective:
- Safe action execution from a single control surface.

In scope:
- `send`, `view-output`, `kill`.
- API v1 write endpoints.
- Shared idempotent request handling (`request_ref`) for all action types.
- Action audit trail via `action_id` correlation.

Entry criteria:
- Phase 1 gates passed.

Exit criteria:
- Control actions reject stale snapshot/runtime.
- Stale action attempts rejected by integration tests.
- Action-to-event correlation queryable by `action_id`.
- Idempotent replay tests pass.

### Phase 2: Gemini + Reliability Hardening

Objective:
- Third adapter support and operational resilience.

In scope:
- Gemini adapter.
- Reconnect/backoff tuning.
- JSON schema hardening and contract compatibility checks.
- Richer list/watch filters and sorting.

Exit criteria:
- All three adapters converge stably under target disconnect/reconnect and event disorder scenarios.

### Phase 2.5: Adapter Expansion

Objective:
- Add new adapters without core redesign.

In scope:
- Copilot CLI adapter (v1).
- Cursor CLI adapter (v1).

Exit criteria:
- Both adapters pass shared adapter contract and state-convergence suites.

### Phase 3: macOS Resident App

Objective:
- Deliver operator UI over daemon API without backend forks.

In scope:
- Global/session/window/pane screens.
- Safe actions parity with CLI.

Exit criteria:
- API v1 compatibility verified for all app actions and watch flows.

## 6. Dependency and Sequence Rules

- No control actions before Phase 1 watch/read contracts are stable.
- No adapter expansion before Phase 2 gate bundle (`TC-033`, `TC-034`, `TC-035`, `TC-046`, `TC-047`, `TC-048`, `TC-052`) is green.
- No app shipping before API v1 compatibility contract and test suite are stable.
- Attach shipment requires action snapshot + idempotency core tests to be green.

## 7. Gate Binding

- Phase close decision is blocked unless the corresponding test bundle in `docs/test-catalog.md` is fully green.
- Any waived test must be explicitly approved and recorded with expiry date.

## 8. Change Control

- Any contract-level change must update:
  - `docs/agtmux-spec.md`
  - `docs/implementation-plan.md`
  - `docs/tasks.md`
  - `docs/test-catalog.md`
- All contract changes require explicit migration and backward-compatibility note.

## 9. Reporting Cadence

- Weekly: phase status, blockers, top risks.
- Per PR: linked task IDs + linked test IDs + gate impact.
- Phase close: gate checklist evidence and residual risk summary.

## 10. Test Execution Profiles

- CI profile:
  - Runs on Linux with tmux (`>= 3.3`) and local ssh test harness.
  - Must execute all CI-labeled tests, including watch/action contracts and target executor flows.
- Nightly profile:
  - Runs multi-target profile (host + ssh targets) with benchmark workload.
  - Must execute Nightly-labeled tests and publish artifacts (logs, metrics, seeds, fixture versions).
- Manual+CI profile:
  - Uses reproducible runbook and evidence artifacts attached to PR.
