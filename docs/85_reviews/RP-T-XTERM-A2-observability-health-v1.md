# Review Pack — T-XTERM-A2: observability + replay ack compaction cross-repo closure

## Objective
- Task: T-XTERM-A2
- Close cross-repo acceptance gate: both remaining acceptance criteria now confirmed satisfied

## Acceptance criteria status

### Criterion 1: agtmux-term A1 consumer passes ui.bootstrap.v2/ui.changes.v2 unchanged
**Status: SATISFIED**

Evidence (from T-XTERM-A3 Phase 3 verification, 2026-03-11):
- `swift test --filter AgtmuxSyncV2DecodingTests` → 10/10 PASS
- `swift test --filter AppViewModelA0Tests/testLiveMarch8BootstrapSampleWithNullExactLocationFieldsFailsClosedAndSurfacesIncompatibleDaemon` → PASS
- Blocker T-XTERM-A3 DONE (producer orphan-pane fix landed)

### Criterion 2: agtmux-term ui.health.v1 consumer connected and surfacing in UI
**Status: SATISFIED**

Evidence (code present in agtmux-term as of 2026-03-11):

| Layer | File | Line |
|-------|------|------|
| Model | `Sources/AgtmuxTermCore/AgtmuxUIHealthModels.swift` | `AgtmuxUIHealthV1: Codable` |
| Client call | `Sources/AgtmuxTermCore/AgtmuxDaemonClient+SyncV2.swift:74` | `method: "ui.health.v1"` |
| XPC contract | `Sources/AgtmuxTermCore/AgtmuxDaemonXPCContract.swift:27` | `fetchUIHealthV1()` |
| Service endpoint | `Sources/AgtmuxDaemonService/ServiceEndpoint.swift:299` | `fetchUIHealthV1()` |
| AppViewModel | `Sources/AgtmuxTerm/AppViewModel.swift:802` | `let health = try await localHealthClient.fetchUIHealthV1()` |
| SidebarView render | `Sources/AgtmuxTerm/SidebarView.swift:968` | `let health: AgtmuxUIHealthV1` |

## Daemon-side implementation summary (already landed)

| Component | File | Evidence |
|-----------|------|---------|
| `ui.health.v1` handler | `crates/agtmux-runtime/src/server.rs:304` | `"ui.health.v1" => build_ui_health_v1(&st)` |
| Health builder | `crates/agtmux-runtime/src/server.rs:723` | `pub(crate) fn build_ui_health_v1(...)` |
| Tests | `crates/agtmux-runtime/src/server.rs:1602` | 3 unit tests + 1 handler test PASS |
| Projection ack | `crates/agtmux-daemon-v5/src/projection.rs:107` | `replay_acked_cursor: ReplayCursor` |

## NOT changed in this closure

- No daemon code changes (implementation was already in place)
- No agtmux-term code changes (consumer was already in place)
- `sync_v2_compat` module itself (Phase 3 schedule unchanged)
- `ui.bootstrap.v2` / `ui.changes.v2` endpoints (T-SV2-P2 schedule unchanged)

## Verification evidence

- `just verify` → PASS (213 tests, 0 failed)
- `cargo fmt --check` PASS
- `cargo clippy --workspace -- -D warnings` PASS

## Review Verdict — GO

- Daemon-side implementation (replay ack compaction, `ui.health.v1`) already committed and tested
- Cross-repo criterion 1: v2 consumer tests pass (10/10 via T-XTERM-A3)
- Cross-repo criterion 2: ui.health.v1 consumer already wired end-to-end in agtmux-term
- Both blockers (T-XTERM-A1, T-XTERM-A3) confirmed DONE
- T-XTERM-A2 is ready to be marked DONE
