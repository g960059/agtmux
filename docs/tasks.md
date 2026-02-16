# AGTMUX Task List (v0.5)

Date: 2026-02-13
Status: Draft
Source Spec: `docs/agtmux-spec.md`
Plan Reference: `docs/implementation-plan.md`

## 1. Task Backlog

| ID | Phase | Priority | Task | FR/NFR | Depends On | Acceptance Criteria | Test IDs | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TASK-001 | 0 | P0 | Add base SQLite migrations for all core tables | FR-3, FR-14, FR-15 | - | Migration applies/rolls back on clean DB | TC-001, TC-002 | Todo |
| TASK-002 | 0 | P0 | Enforce active runtime partial unique index | FR-13 | TASK-001 | Only one `ended_at IS NULL` runtime per pane | TC-003 | Todo |
| TASK-003 | 0 | P0 | Implement runtime identity lifecycle (`tmux_server_boot_id`, `pane_epoch`) | FR-13 | TASK-001 | Runtime rollover updates epoch and rejects stale refs | TC-004, TC-005 | Todo |
| TASK-004 | 0 | P0 | Implement ordering/dedupe comparator and ingestion idempotency path | FR-14, NFR-4, NFR-8 | TASK-001 | Deterministic output for shuffled identical streams and duplicate-safe apply | TC-006, TC-007 | Todo |
| TASK-005 | 0 | P0 | Implement `event_inbox` bind resolver | FR-14 | TASK-001, TASK-003 | Pending bind only resolves on safe candidate | TC-008, TC-009 | Todo |
| TASK-006 | 0 | P1 | Implement reconciler stale/health transitions | FR-8, NFR-2 | TASK-004 | Down/stale signals converge to unknown safely | TC-010 | Todo |
| TASK-007 | 0 | P1 | Add payload redaction + retention jobs | NFR-9 | TASK-001 | Unredacted payload never persisted in SQLite | TC-011, TC-012, TC-050 | Todo |
| TASK-008 | 0 | P1 | Add index set from spec baseline | NFR-1, NFR-3 | TASK-001 | Query plan confirms expected index usage | TC-013 | Todo |
| TASK-009 | 1 | P0 | Implement target manager commands | FR-10 | TASK-001, TASK-031 | add/connect/list/remove works for local + ssh targets | TC-014 | Todo |
| TASK-010 | 1 | P0 | Implement Claude adapter | FR-1, FR-2 | TASK-003, TASK-004 | Claude signals normalize into canonical states | TC-015 | Todo |
| TASK-011 | 1 | P0 | Implement Codex adapter | FR-1, FR-2 | TASK-003, TASK-004 | Codex notify/wrapper events normalize correctly | TC-016 | Todo |
| TASK-012 | 1 | P0 | Implement API v1 read endpoints | FR-4, FR-6, FR-7, FR-9 | TASK-004, TASK-006 | panes/windows/sessions JSON contract stable with grouping counts and multi-target aggregate semantics | TC-017 | Todo |
| TASK-013 | 1 | P0 | Implement watch stream (`stream_id`, `cursor`) | FR-4, FR-6 | TASK-012 | Resume semantics pass restart/expiry tests | TC-019, TC-020, TC-051 | Todo |
| TASK-014 | 1 | P0 | Implement CLI list/watch mapping to API v1 | FR-4, FR-6 | TASK-012, TASK-013 | CLI output and JSON align with API schema | TC-021 | Todo |
| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012, TASK-018 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
| TASK-016 | 1 | P1 | Implement partial-result response envelope | NFR-7 | TASK-012 | `partial`, `target_errors`, requested/responded targets emitted | TC-023 | Todo |
| TASK-017 | 1 | P1 | Canonicalize `target-session` encoding rules in CLI/API | FR-16 | TASK-012 | encoded session names round-trip safely | TC-024 | Todo |
| TASK-018 | 1 | P0 | Implement shared `actions` write path with idempotency | FR-15, NFR-4 | TASK-003, TASK-012 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
| TASK-019 | 1.5 | P0 | Implement `send` with `text/stdin/key/paste` modes | FR-5 | TASK-018 | mode behaviors match spec and guard checks | TC-027 | Todo |
| TASK-020 | 1.5 | P0 | Implement `view-output` action | FR-5 | TASK-018 | bounded capture returns correct pane output | TC-028 | Todo |
| TASK-021 | 1.5 | P0 | Implement `kill` mode `key\|signal` and guard logic | FR-5, FR-15 | TASK-018 | pid missing under signal mode yields `E_PID_UNAVAILABLE` | TC-029, TC-030 | Todo |
| TASK-022 | 1.5 | P1 | Implement action-event audit correlation | FR-15 | TASK-018 | action_id traces to correlated events | TC-031 | Todo |
| TASK-023 | 1 | P1 | Implement structured error envelope + code mapping | FR-16 | TASK-012, TASK-018 | machine-readable error object stable across API/CLI | TC-018, TC-032, TC-053 | Todo |
| TASK-024 | 2 | P0 | Implement Gemini adapter | FR-1, FR-2 | TASK-004, TASK-006 | Gemini parser/wrapper transitions converge | TC-033 | Todo |
| TASK-025 | 2 | P1 | Reliability hardening for reconnect/backoff | NFR-1, NFR-7 | TASK-024 | sustained target flaps keep system convergent | TC-034, TC-052 | Todo |
| TASK-026 | 2 | P1 | JSON schema compatibility tests for v1 | FR-6 | TASK-012, TASK-013, TASK-023 | schema compatibility suite gates release | TC-035, TC-045 | Todo |
| TASK-027 | 2.5 | P1 | Add Copilot CLI adapter | FR-12 | TASK-025, TASK-026, TASK-035 | adapter passes shared contract tests | TC-036 | Todo |
| TASK-028 | 2.5 | P1 | Add Cursor CLI adapter | FR-12 | TASK-025, TASK-026, TASK-035 | adapter passes shared contract tests | TC-037 | Todo |
| TASK-029 | 3 | P1 | Build macOS app read views using API v1 | Goal | TASK-012, TASK-013 | app can render global/session/window/pane views | TC-038 | Todo |
| TASK-030 | 3 | P1 | Build macOS app actions with same safety checks | Goal, FR-15 | TASK-018, TASK-019, TASK-021 | app actions preserve fail-closed semantics | TC-039 | Todo |
| TASK-031 | 0 | P0 | Implement `TargetExecutor` and daemon boundary | FR-9 | TASK-001 | all target read/write paths go through executor abstraction with UDS transport + health endpoint contract | TC-040 | Todo |
| TASK-032 | 0 | P0 | Implement tmux topology observer per target | FR-3, FR-9 | TASK-031 | topology snapshots converge across targets | TC-041 | Todo |
| TASK-033 | 1 | P1 | Implement grouping and summary rollups | FR-7 | TASK-012 | session/window summaries match spec section 7.6 | TC-042 | Todo |
| TASK-034 | 1 | P1 | Enforce aggregated multi-target response semantics | FR-9, NFR-7 | TASK-009, TASK-012, TASK-016 | requested/responded/target_errors consistency | TC-043 | Todo |
| TASK-035 | 2 | P0 | Implement adapter registry capability-driven dispatch | FR-11, NFR-5 | TASK-024 | add adapter without core engine changes | TC-046 | Todo |
| TASK-036 | 2 | P1 | Add adapter contract version compatibility checks | NFR-6 | TASK-035 | backward-compatible minor version changes validated | TC-047 | Todo |
| TASK-037 | 2 | P1 | Add richer list/watch filters and sorting | FR-4, FR-7 | TASK-012, TASK-013 | filter/sort contract remains stable and deterministic | TC-048 | Todo |
| TASK-038 | 0 | P1 | Harden ingestion duplicate storm behavior | NFR-4, FR-14 | TASK-004, TASK-005 | duplicate/retry storms do not create divergent state | TC-049 | Todo |
| TASK-039 | 1 | P1 | Add visibility latency benchmark harness and SLO gate | NFR-1 | TASK-012, TASK-013, TASK-031 | benchmark profile is reproducible and enforces p95 visible lag <= 2s | TC-044 | Todo |
| TASK-040 | 0 | P0 | Establish CI/Nightly execution baseline (tmux + local sshd) | NFR-1, NFR-7 | TASK-001, TASK-031 | CI runs tmux/target integration suite and Nightly runs multi-target/reliability suites with artifacts | TC-040, TC-041, TC-044, TC-052 | Todo |

## 2. Immediate Sprint Candidates (Recommended)

- TASK-001
- TASK-002
- TASK-003
- TASK-004
- TASK-005
- TASK-006
- TASK-031
- TASK-032
- TASK-040

## 3. Definition of Done

A task is Done only when:

- Acceptance criteria are met.
- Linked CI tests are automated and passing.
- Linked Nightly tests are green in the latest scheduled run.
- Manual+CI tests include reproducible execution evidence in PR.
- Any API/contract changes are reflected in spec and test catalog.
- Operational logging/error surfaces are documented when behavior changed.

## 4. Phase 9 Backlog (Interactive TTY)

Source Strategy:
- `docs/implementation-records/phase9-main-interactive-tty-strategy-2026-02-16.md`
- `docs/implementation-records/phase9-task-decomposition-and-tdd-2026-02-16.md`

| ID | Phase | Priority | Task | FR/NFR | Depends On | Acceptance Criteria | Test IDs | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TASK-901 | 9 | P0 | Define terminal streaming API contract (`attach/detach/write/resize/stream`) | FR-1, FR-2, FR-9 | - | API draft reviewed and approved | TC-901 | Todo |
| TASK-902 | 9 | P0 | Build daemon proxy PTY PoC for local target | FR-1, FR-2 | TASK-901 | key-by-key input and output stream proven | TC-902, TC-903 | Todo |
| TASK-903 | 9 | P0 | Evaluate terminal emulator integration for macapp (SwiftTerm-first) | FR-2, FR-4 | TASK-902 | key/IME/resize feasibility documented | TC-902, TC-908 | Todo |
| TASK-904 | 9 | P0 | Measure Step0 SLO feasibility (`input/switch` latency) | NFR-1, NFR-2 | TASK-902 | p95 feasibility report produced | TC-916 | Todo |
| TASK-911 | 9 | P0 | Implement daemon `TerminalSessionManager` with lifecycle and GC | FR-7, NFR-3 | TASK-904 | attach/detach lifecycle + TTL/GC pass | TC-902, TC-915 | Todo |
| TASK-912 | 9 | P0 | Implement `/v1/terminal/attach|detach|write|resize` endpoints | FR-1, FR-2, FR-9 | TASK-911 | endpoints pass contract and stale guard checks | TC-902, TC-903, TC-907 | Todo |
| TASK-913 | 9 | P0 | Implement `/v1/terminal/stream` long-lived endpoint | FR-2, FR-7 | TASK-912 | streaming frames are contract-compatible | TC-902, TC-905 | Todo |
| TASK-921 | 9 | P0 | Implement persistent UDS terminal client in macapp | FR-2, NFR-1 | TASK-913 | no per-key CLI process launch in primary path | TC-902 | Todo |
| TASK-922 | 9 | P0 | Implement `TerminalSessionController` (interactive + snapshot backend) | FR-7, FR-8 | TASK-921 | attach/retry/degrade flows pass tests | TC-904, TC-905 | Todo |
| TASK-923 | 9 | P1 | Integrate stale runtime/state guard in all write paths | FR-9 | TASK-922 | stale writes rejected with recovery path | TC-907 | Todo |
| TASK-924 | 9 | P0 | Implement resize conflict policy for multi-client tmux sessions | FR-6 | TASK-923 | resize behavior matches documented policy under concurrent clients | TC-909 | Todo |
| TASK-931 | 9 | P0 | Add interactive terminal view with feature-flagged rollout | FR-1, FR-4 | TASK-924 | interactive/snapshot UI switch works | TC-904, TC-908 | Todo |
| TASK-932 | 9 | P1 | Add always-available external terminal fallback action | FR-8 | TASK-931 | fallback command is reachable and works | TC-906, TC-917 | Todo |
| TASK-941 | 9 | P0 | Implement terminal recovery and automatic snapshot degrade | FR-7, FR-8 | TASK-932 | consecutive attach failures degrade safely | TC-905 | Todo |
| TASK-942 | 9 | P1 | Implement interactive kill switch and rollback path | FR-8 | TASK-941 | one switch reverts to snapshot mode | TC-914 | Todo |
| TASK-951 | 9 | P0 | Add daemon contract/integration tests for terminal APIs | FR-2, FR-9 | TASK-942 | Go contract suite stable in CI | TC-902, TC-903, TC-907, TC-915 | Todo |
| TASK-952 | 9 | P0 | Add macapp unit/UI tests for switching/recovery/fallback | FR-5, FR-7, FR-8 | TASK-951 | Swift tests cover race and degrade paths | TC-904, TC-905, TC-917 | Todo |
| TASK-953 | 9 | P1 | Add performance/leak tests and dashboards | NFR-1, NFR-2, NFR-3 | TASK-952 | latency/leak gates are measurable and enforced | TC-910, TC-911, TC-912 | Todo |
| TASK-961 | 9 | P0 | Rollout local target as default interactive mode | Goal | TASK-953 | local satisfies AC set | TC-911, TC-912 | Todo |
| TASK-962 | 9 | P1 | Rollout ssh targets in staged mode | Goal | TASK-961 | ssh parity and fallback confirmed | TC-913 | Todo |
