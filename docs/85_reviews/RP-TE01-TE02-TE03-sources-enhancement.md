# Review Pack: T-E01 / T-E02 / T-E03 — Sources Enhancement (Phase 8)

## Objective
- Tasks: T-E01 (Hooks coverage expansion), T-E02 (JSONL fd-based discovery), T-E03 (setup-hooks --check)
- Spec: FR-053〜FR-059 (`docs/20_spec.md`)
- Design: `docs/40_design.md` §Hooks, §JSONL discovery, §OSC Tap
- ADR: `docs/80_decisions/ADR-20260301-osc-architecture.md`

## Summary
1. **T-E01**: Added 6 new Claude Code hook types (SessionStart/SessionEnd/PermissionRequest/UserPromptSubmit/PostToolUseFailure/PreCompact) to translation and setup-hooks config. Added transcript_path forwarding — SessionStart hook payloads carry the JSONL path, which is cached in `DaemonState.transcript_path_hints` and used as P1 JSONL discovery source (highest priority), overriding CWD-based P3.
2. **T-E02**: Added `PaneDiscoveryHint` struct replacing the tuple in `discover_sessions`. Added `discover_jsonl_via_lsof` (P2 tier) that runs `lsof -F n -p {pid}` to find open JSONL files directly from the pane process. P2 overrides P3 (CWD-based), P1 (transcript_path) still overrides P2.
3. **T-E03**: Added `--check` flag to `agtmux setup-hooks`. Reads settings.json and shows registered/missing status per hook type. Exits 0 if all OK, exits 1 if any missing.

## Change scope

| File | Change |
|------|--------|
| `crates/agtmux-source-claude-hooks/src/translate.rs` | 6 new hook type mappings in `normalize_event_type`, 6 new test cases |
| `crates/agtmux-runtime/src/setup_hooks.rs` | HOOK_TYPES: 5→11; added `HookStatus`, `HookCheckResult`, `check_hooks()`, 3 tests |
| `crates/agtmux-source-claude-jsonl/src/discovery.rs` | Added `PaneDiscoveryHint`, made `session_id_from_jsonl_path` public, added `discovery_from_transcript_path()`, `discover_jsonl_via_lsof()`, updated `discover_sessions()` signature, updated tests |
| `crates/agtmux-source-claude-jsonl/src/source.rs` | Updated `discover_sessions` call site to use `PaneDiscoveryHint` |
| `crates/agtmux-runtime/src/poll_loop.rs` | Added `transcript_path_hints` to `DaemonState`; Step 8b: scan events for transcript_path hints; Step 6b: use `PaneDiscoveryHint`, overlay P1 hints |
| `crates/agtmux-runtime/src/cli.rs` | Added `--check` flag to `SetupHooksOpts` |
| `crates/agtmux-runtime/src/main.rs` | Branch on `opts.check` in SetupHooks arm |

## Verification evidence

- `just verify` run at end of each task: PASS
- T-E01: 756 tests, 0 failed
- T-E02: 760+ tests, 0 failed (includes new lsof-nonexistent-pid test)
- T-E03: 760+ tests, 0 failed (includes 3 new HookCheckResult tests)
- `cargo fmt --check`: PASS all tasks
- `cargo clippy -D warnings`: PASS all tasks (only pre-existing warnings)

## Risk declaration

- **Breaking change**: `discover_sessions` signature changed from tuple slice to `&[PaneDiscoveryHint]` — source.rs updated accordingly. No external callers (crate-internal API).
- **Fallbacks**: `discover_jsonl_via_lsof` returns `None` if lsof unavailable or pid not found → falls through to CWD-based P3. transcript_path hints are only used if the file exists. Both are fail-safe.
- **lsof blocking**: The `lsof` subprocess call is blocking, consistent with existing fs I/O in `discover_sessions`. Acceptable for MVP.
- **One-tick delay for transcript_path**: SessionStart events arrive at Step 8b but JSONL discovery runs at Step 6b (earlier in the same tick). Transcript_path hint is therefore used starting from the next tick (~1s delay). Acceptable.
- **Known gaps**: lsof process tree not fully traversed (only `pane_pid` directly); child processes not checked. Sufficient for MVP since Claude Code keeps its JSONL file open in the main node process.

## Reviewer request
- Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
- If NEED_INFO: list up to 3 concrete missing items + why required
