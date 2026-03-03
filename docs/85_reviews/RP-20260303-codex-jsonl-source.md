# Review Pack — RP-20260303-codex-jsonl-source

## Objective
- Tasks: T-codex01a, T-codex01b, T-codex01c (Phase 9)
- Goal: Replace broken mtime/App-Server Codex detection with semantic JSONL FSM parsing.
  Root cause: wrong JSON key (`.data.type` vs `.payload.type`), wrong event names,
  keepalive-write hypothesis was false, App Server only returns historical sessions.

## Summary
1. Created `crates/agtmux-source-codex-jsonl/` — new 5-module crate mirroring `claude-jsonl` pattern.
2. Added `SourceKind::CodexJsonl` (Deterministic tier) to `agtmux-core-v5/src/types.rs`.
3. FSM: `Init → Running → ToolExecuting / WaitingApproval / WaitingInput → Ended`.
   Driven by `.payload.type` key; correct event names from analysis of 1130 real JSONL files.
4. Gutted `codex_poller.rs` (700 lines → 4-line stub); replaced Step 6a (180 lines) in poll_loop.rs.
5. Deleted `DaemonState` fields: `codex_appserver_client`, `codex_supervisor`, `codex_capture_tracker`, `codex_source`.
6. Gateway source set updated: `CodexAppserver` → `CodexJsonl`.

## Change scope
| File | Change |
|------|--------|
| `crates/agtmux-source-codex-jsonl/` | NEW crate (5 modules, ~600 lines + 46 tests) |
| `crates/agtmux-core-v5/src/types.rs` | Added `SourceKind::CodexJsonl` |
| `Cargo.toml` (workspace) | Added `agtmux-source-codex-jsonl` member + dep |
| `crates/agtmux-runtime/Cargo.toml` | Replaced `codex-appserver` with `codex-jsonl` |
| `crates/agtmux-runtime/src/codex_poller.rs` | Gutted to 4-line stub |
| `crates/agtmux-runtime/src/poll_loop.rs` | Step 6a/6a-bis replaced; DaemonState refactored; Step 8a updated |
| `crates/agtmux-runtime/src/server.rs` | Removed `codex_appserver` source kind arm |
| `scripts/tests/e2e/scenarios/codex-semantic-states.sh` | NEW e2e scenario (synthetic JSONL injection) |

## Verification evidence
- `cargo fmt --all` → PASS (zero diffs)
- `cargo clippy --workspace -- -D warnings` → PASS (zero warnings)
- `cargo test --workspace` → PASS (800+ tests, 0 failed)
- `just verify` → PASS (full pipeline)
- Test count breakdown: 140 (agtmux-runtime) + 185 (core) + 145 + 92 + 14 + 43 + 17 + 50 (codex-jsonl) + 68 + 46

## Risk declaration
- Breaking change: **yes** — `SourceKind::CodexAppserver` removed from Gateway; `codex_appserver` source hook endpoint now returns error.
  Any external code using `codex_appserver` source kind will need to migrate to `codex_jsonl`.
- Fallbacks: **none** (by design — old App Server / mtime code fully deleted)
- Known gaps / follow-ups:
  - `codex_poller.rs` stub should be deleted in a follow-up (currently kept to avoid module declaration noise)
  - `agtmux-source-codex-appserver` crate still in workspace but unused — can be removed in follow-up
  - e2e `codex-semantic-states.sh` tests against live `~/.codex/sessions/` — requires `just preflight-online`
  - T-E04 (OSC 9;4 tap via pipe-pane) still pending (Post-MVP)
  - WaitingInput → Ended via process-exit detection not yet implemented (600s staleness timeout only)

## Reviewer request
- Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
- Focus areas: FSM correctness, discovery.rs CWD matching, DaemonState field removal completeness, server.rs source kind handling.

---

## Review Result

**Reviewer**: Claude Sonnet 4.6 (code review subagent)
**Date**: 2026-03-03

### Blocking Issues

None.

### Non-blocking Issues

1. **fsm.rs:67-68 — `WaitingApproval → task_complete` falls through to `_` (no-op)**
   `task_complete` from `WaitingApproval` returns `WaitingApproval` unchanged. If approval is rejected internally (no `exited_review_mode` emitted) and `task_complete` fires, the session hangs in `WaitingApproval` until the 600s staleness timeout. This is an edge case from real Codex behaviour but should be documented; no code change strictly required now.

2. **discovery.rs:59-60 — `unwrap_or_else` on lsof failure is intentional fallback, not silent**
   `get_cwd_via_lsof` returns `None` on failure and the code falls back to `hint.cwd` (tmux CWD). This is explicitly documented and not a silent error — it is a designed two-tier fallback. No issue; noting for clarity.

3. **discovery.rs:88-135 — `build_session_index` walks all date dirs every poll**
   The full YYYY/MM/DD tree is re-scanned on every poll tick (no mtime-based skip). For users with thousands of historical sessions this is an O(n) I/O cost per tick. Acceptable for MVP; recommend an index-with-cutoff in a follow-up.

4. **source.rs:174-194 — Duplicate heartbeat emission path**
   The bootstrap branch (`!watcher.is_bootstrapped()`) and the post-bootstrap else branch both emit `idle_heartbeat` unconditionally when `!emitted_real_event`. The logic is correct but the two branches are structurally identical — could be collapsed after `mark_bootstrapped()`. Not a bug; style only.

5. **watcher.rs:145-149 — Shared temp dir across watcher tests**
   `temp_jsonl` always returns the same `agtmux-test-codex-watcher` directory with different filenames, but `create_dir_all` may race between parallel test threads. File-level isolation is fine in practice but worth noting.

### Missing Tests

- **FSM: `WaitingApproval + task_complete → WaitingApproval` (no-op)**. The current suite tests `task_complete` from `Running` and `ToolExecuting` but not from `WaitingApproval`. Add a test asserting state remains `WaitingApproval` to lock in the current behaviour and prevent accidental breakage.
- **discovery.rs: macOS `/tmp` → `/private/tmp` canonicalization**. `canonicalize_path` is exercised only via the filesystem (if the path exists). A unit test passing `"/tmp/foo"` and asserting `"/private/tmp/foo"` would make the fallback branch regression-proof.

### Verdict

**GO_WITH_CONDITIONS**

Conditions (register as follow-up tasks):
1. Add FSM test: `WaitingApproval + task_complete` should remain `WaitingApproval` (non-blocking correctness gap in test suite).
2. Add `canonicalize_path` unit test for `/tmp` → `/private/tmp` substitution path.

Core implementation is correct: FSM transitions are sound and fully covered for the happy paths, `lsof -p <pid> -d cwd -Fn` parsing correctly guards against macOS outputting all-process CWDs (PID block prefix check at discovery.rs:191-206), DaemonState old fields are fully removed, `SourceKind::CodexJsonl` is wired correctly into Gateway, and `source.ingest` for `codex_appserver` fails loudly with -32602. The 50 codex-jsonl tests plus the runtime integration test (`poll_tick_pulls_from_codex_jsonl_source`) provide adequate regression coverage for the main scenarios.
