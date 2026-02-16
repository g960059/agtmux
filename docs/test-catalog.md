# AGTMUX Test Catalog (v0.5)

Date: 2026-02-13
Status: Draft
Source Spec: `docs/agtmux-spec.md`
Plan Reference: `docs/implementation-plan.md`
Task Reference: `docs/tasks.md`

## 1. Test Strategy

Test layers:

- Unit tests for parser/comparator/validation logic.
- Property tests for ordering determinism and replay invariants.
- Integration tests for DB + daemon + adapters.
- Contract tests for API/CLI schemas and error envelopes.
- End-to-end tests for multi-target workflows.
- Performance and resilience tests for latency and partial-failure behavior.

## 2. Contract Test Matrix

| Test ID | Layer | Contract | Source Requirement | Scenario | Pass Criteria | Automation |
| --- | --- | --- | --- | --- | --- | --- |
| TC-001 | Integration | Base migration correctness | FR-3 | Apply fresh migration and rollback | schema applies/rolls back cleanly | CI |
| TC-002 | Integration | Core table constraints | FR-14, FR-15 | Validate PK/UNIQUE/FK constraints | invalid writes rejected | CI |
| TC-003 | Integration | Active runtime uniqueness | FR-13 | Concurrent runtime inserts on same pane | max one active runtime | CI |
| TC-004 | Unit | Runtime identity generation | FR-13 | epoch increment triggers and runtime rollover | stale runtime rejected | CI |
| TC-005 | Integration | Stale runtime guard | FR-13, FR-15 | action against old runtime after pane reuse | `E_RUNTIME_STALE` | CI |
| TC-006 | Property | Ordering determinism | FR-14, NFR-8 | shuffle same event set repeatedly | identical final state hash | CI |
| TC-007 | Unit | Dedupe behavior | FR-14, NFR-4 | duplicate event submission | single logical apply | CI |
| TC-008 | Integration | Pending bind safe resolution | FR-14 | candidate exists with matching hints | inbox moves to `bound` | CI |
| TC-009 | Integration | Pending bind rejection path | FR-14 | no candidate / ambiguous / TTL expiry | `dropped_unbound` + reason_code | CI |
| TC-010 | Integration | Unknown-safe convergence | FR-8, NFR-2 | target down and stale signals | state becomes `unknown/*` | CI |
| TC-011 | Integration | Payload redaction | NFR-9 | ingest sensitive payload sample | stored payload is redacted | CI |
| TC-012 | Integration | Retention purge safety | NFR-9 | run retention jobs | expired rows removed, remaining integrity intact | Nightly |
| TC-013 | Performance | Index baseline utility | NFR-1, NFR-3 | profile hot queries | index-backed plan, acceptable latency | Nightly |
| TC-014 | E2E | Target manager basic flow | FR-10 | add/connect/list/remove targets | all commands succeed with expected output | CI |
| TC-015 | Integration | Claude state normalization | FR-1, FR-2 | hook event fixtures | canonical states correct | CI |
| TC-016 | Integration | Codex state normalization | FR-1, FR-2 | notify/wrapper fixtures | canonical states correct | CI |
| TC-017 | Contract | API read schema | FR-4, FR-6, FR-7, FR-9 | `/v1/panes\|windows\|sessions` responses with grouping and multi-target aggregation | required fields, identity shape, grouping counts, and aggregate semantics are stable | CI |
| TC-018 | Contract | API/CLI error envelope shape | FR-16 | invalid refs/cursors/actions | code/message/details schema stable | CI |
| TC-019 | E2E | Watch cursor resume | FR-4, FR-6 | resume stream with valid cursor | no gaps, no duplicates | CI |
| TC-020 | E2E | Watch reset on expired cursor | FR-4, FR-6 | request stale cursor | reset+snapshot behavior emitted | CI |
| TC-021 | Contract | CLI/API parity for list/watch | FR-4, FR-6 | compare CLI json and API json | semantic parity | CI |
| TC-022 | E2E | Attach fail-closed | FR-5, FR-15 | stale snapshot/runtime attach | request rejected | CI |
| TC-023 | Contract | Partial-result envelope | NFR-7 | one target fails in aggregated read | `partial` and `target_errors` present | CI |
| TC-024 | E2E | Encoded target-session round-trip | FR-16 | session names with `/`, `%`, spaces | filter/ref resolution correct | CI |
| TC-025 | Integration | Action idempotent replay | FR-15, NFR-4 | resend same request_ref | same action_id and result | CI |
| TC-026 | Integration | Idempotency conflict | FR-15, NFR-4 | same key with different payload | `E_IDEMPOTENCY_CONFLICT` | CI |
| TC-027 | E2E | Send action modes | FR-5, FR-15 | text/stdin/key/paste flows | expected tmux behavior + guard checks | CI |
| TC-028 | E2E | View-output bounds | FR-5 | capture with line limit | bounded output correctness | CI |
| TC-029 | E2E | Kill mode key | FR-5 | INT via key mode | graceful interrupt path works | CI |
| TC-030 | E2E | Kill mode signal validation | FR-5 | signal mode without pid | `E_PID_UNAVAILABLE` | CI |
| TC-031 | Integration | Action-event correlation | FR-15 | execute action and inspect events | traceable by action_id | CI |
| TC-032 | Contract | Error code mapping consistency | FR-16 | API + CLI error scenarios | stable mapping for automation | CI |
| TC-033 | Integration | Gemini adapter convergence | FR-1, FR-2 | wrapper/parser fixtures + disorder | stable canonical convergence | CI |
| TC-034 | Resilience | Reconnect/backoff behavior | NFR-1, NFR-7 | repeated target flaps | no deadlock, recovers within SLO | Nightly |
| TC-035 | Contract | JSON schema compatibility | FR-6 | compare schema snapshots across commits | backward-compatible changes only | CI |
| TC-036 | Integration | Copilot adapter contract suite | FR-12 | adapter integration fixtures | core engine unchanged | CI |
| TC-037 | Integration | Cursor adapter contract suite | FR-12 | adapter integration fixtures | core engine unchanged | CI |
| TC-038 | E2E | macOS app read parity | Goal | app screens vs API v1 data | parity validated | Manual+CI |
| TC-039 | E2E | macOS app action safety parity | Goal, FR-15 | app actions under stale runtime | same fail-closed behavior as CLI | Manual+CI |
| TC-040 | Integration | TargetExecutor and daemon boundary | FR-9 | mixed local/ssh target read-write flows over daemon UDS API | all target operations go through executor boundary and `/v1/health` contract is stable | CI |
| TC-041 | E2E | Multi-target topology observer | FR-3, FR-9 | target reconnect and pane churn | topology converges without stale bleed | CI |
| TC-042 | Contract | Grouping and summary correctness | FR-7 | panes/windows/sessions rollups | counts and precedence are correct | CI |
| TC-043 | Contract | Aggregated multi-target semantics | FR-9, NFR-7 | partial target failure during aggregated read | requested/responded/target_errors consistency | CI |
| TC-044 | Performance | Visibility latency benchmark | NFR-1 | benchmark profile traffic | visible lag p95 <= 2s and benchmark artifacts are emitted | Nightly |
| TC-045 | Contract | Watch JSONL schema compatibility | FR-6 | compare watch snapshot/delta schemas across commits | schema remains compatible | CI |
| TC-046 | Integration | Adapter registry extensibility | FR-11, NFR-5 | add mock adapter through registry | no core engine modifications required | CI |
| TC-047 | Contract | Adapter contract version compatibility | NFR-6 | adapter minor version bump | backward-compatible behavior preserved | CI |
| TC-048 | Contract | List/watch filter and sort stability | FR-4, FR-7 | filter+sort combinations | deterministic order and stable schema | CI |
| TC-049 | Integration | Duplicate-storm convergence | FR-14, NFR-4 | replay duplicate/retry bursts | no divergent state or double apply | CI |
| TC-050 | Security | Debug raw payload prohibition | NFR-9 | run debug mode with secret-like payloads | unredacted payload never lands in SQLite | CI |
| TC-051 | Resilience | Watch continuity after daemon restart | FR-4, NFR-7 | restart during active watch stream | restart resumes with reset/snapshot without loss | CI |
| TC-052 | Resilience | SQLite busy/lock recovery | NFR-3, NFR-7 | injected lock contention | retries/backoff preserve consistency | Nightly |
| TC-053 | Contract | Full error code matrix regression | FR-16 | enumerate all defined error codes | API/CLI code mapping complete and stable | CI |

## 3. Phase Gate Bundles

| Gate | Required Tests |
| --- | --- |
| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010, TC-011, TC-012, TC-013, TC-040, TC-041, TC-049, TC-050 |
| Phase 1 close | Phase 0 bundle + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024, TC-042, TC-043, TC-044, TC-045, TC-051 |
| Phase 1.5 close | Phase 1 bundle + TC-025, TC-026, TC-027, TC-028, TC-029, TC-030, TC-031, TC-032, TC-053 |
| Phase 2 close | Phase 1.5 bundle + TC-033, TC-034, TC-035, TC-046, TC-047, TC-048, TC-052 |
| Phase 2.5 close | Phase 2 bundle + TC-036, TC-037 |
| Phase 3 close | Phase 2.5 bundle + TC-038, TC-039 |

## 4. Benchmark Profile (for NFR-1)

Default benchmark profile for visibility latency gates:

- Targets: 3 (host + 2 ssh targets)
- Active panes: 60 total
- Event ingest rate: 10 events/second sustained
- Budget: p95 visible lag <= 2 seconds

## 5. Test Data and Fixtures

- Synthetic ordered/disordered event fixtures per adapter.
- Pending-bind ambiguity fixtures with pane reuse timelines.
- Cursor expiry fixtures with retention-window cutoff.
- Security fixtures containing secret-like payload strings.
- Multi-target reconnect fixtures with clock-skew variations.

## 6. Reproducibility Rules

- Property tests must pin random seed and report it in failure output.
- Time-sensitive tests must use fixed/fake clock unless explicitly marked as wall-clock tests.
- Fixtures must include version/hash metadata.
- Nightly failures must keep artifacts for replay (logs, seed, fixture versions).
- Manual+CI tests must include reproducible runbook evidence in PR.

## 7. Execution Environment Contract

- CI environment:
  - Linux runner with tmux (`>= 3.3`) and local sshd harness.
  - Must run all CI-labeled tests in this catalog.
  - tmux session names and socket paths must be test-unique for parallel runs.
- Nightly environment:
  - Multi-target profile (host + ssh targets) with benchmark workload from Section 4.
  - Must run all Nightly-labeled tests and retain artifacts for replay.
  - Required artifacts: logs, metrics, property seeds, fixture version/hash.
- Manual+CI environment:
  - Must attach reproducible runbook and evidence artifacts to PR.

## 8. Reporting and Traceability

Every PR affecting runtime/API/action behavior must include:

- Referenced task IDs from `docs/tasks.md`.
- Referenced test IDs from this catalog.
- Gate impact statement (`which phase gate is affected`).

## 9. Phase 9 Test Matrix (Interactive TTY)

| Test ID | Layer | Contract | Source Requirement | Scenario | Pass Criteria | Automation |
| --- | --- | --- | --- | --- | --- | --- |
| TC-901 | Contract | Terminal streaming API draft completeness | FR-1, FR-2, FR-9 | Validate request/response/frame schema for `attach/detach/write/resize/stream` | required fields, error codes, lifecycle states defined | CI |
| TC-902 | Integration | Attach + stream lifecycle | FR-1, FR-2, FR-7 | attach -> output stream -> detach | lifecycle frames emitted in order without leak | CI |
| TC-903 | Integration | key-by-key write round-trip | FR-2 | send control/printable keys through write endpoint | tmux pane output reflects intended keys | CI |
| TC-904 | E2E | Pane switching race safety | FR-5 | switch panes 100 times at 50ms interval | selected pane and rendered pane never diverge | CI |
| TC-905 | Integration | Recovery and degrade policy | FR-7, FR-8 | force attach failures repeatedly | exponential retry then snapshot degrade | CI |
| TC-906 | E2E | External terminal fallback action | FR-8 | invoke fallback from app while interactive fails | external terminal attach command launches correctly | Manual+CI |
| TC-907 | Contract | Write stale guard enforcement | FR-9 | write with stale runtime/state refs | `E_RUNTIME_STALE` or equivalent rejection | CI |
| TC-908 | E2E | IME and modifier key behavior | FR-2, FR-4 | Japanese IME compose/commit + modifiers | no key loss/corruption during interactive session | Manual+CI |
| TC-909 | Integration | Resize conflict behavior | FR-6 | resize with concurrent external clients | behavior matches documented tmux policy | CI |
| TC-910 | Resilience | Long-run terminal session leak check | NFR-3 | 8-hour attach/detach workload | memory/fd growth stays within threshold | Nightly |
| TC-911 | Performance | Input latency SLO | NFR-1 | measure key event -> echo latency (local) | p95 <= 150ms | Nightly |
| TC-912 | Performance | Pane switch latency SLO | NFR-2 | measure selection -> first correct frame (local) | p95 <= 500ms | Nightly |
| TC-913 | E2E | SSH target parity and fallback | Goal | run interactive on ssh target with degradation path | parity or documented fallback without breakage | Manual+CI |
| TC-914 | Integration | Interactive kill switch rollback | FR-8 | toggle interactive off during runtime | app returns to snapshot mode safely | CI |
| TC-915 | Integration | Terminal state/session GC | NFR-3 | churn sessions and stale stream states | stale state entries removed by TTL/GC | CI |
| TC-916 | Performance | Step0 SLO feasibility smoke | NFR-1, NFR-2 | short-run local PoC latency check | feasibility threshold reported in CI artifact | CI |
| TC-917 | Unit | External fallback command resolution | FR-8 | build and validate fallback command parameters | command target/pane binding is deterministic and safe | CI |

### Phase 9 Gate Bundle

| Gate | Required Tests |
| --- | --- |
| Phase 9 Step0 gate | TC-901, TC-902, TC-903, TC-916 |
| Phase 9 close | Phase 9 Step0 gate + TC-905, TC-906, TC-907, TC-908, TC-909, TC-910, TC-913, TC-914, TC-915 |
