# Review Pack — T-XTERM-A6: app-launched explicit `--tmux-socket` zero-managed-bootstrap handback

## Objective
- Task: T-XTERM-A6
- Root cause: app-like sanitized env caused `tmux list-panes -F` to emit `_`-delimited rows; daemon only parsed tab-delimited output, so inventory always failed under the sanitized child env
- All producer-side fixes (A6b–A6h) and explicit-socket scenario suite now confirm no zero-bootstrap

## Phase summary

| Phase | Status | Evidence |
|-------|--------|---------|
| Phase 1: failing repro added | DONE | `explicit-tmux-socket-app-child-late-server.sh` wired into run-all.sh |
| Phase 2: pipe delimiter fix | DONE (already landed) | `pane_info.rs` uses `LIST_PANES_DELIMITER = '|'`; legacy tab fallback preserved; 15/15 tests green |
| Phase 3: explicit-socket producer repros all PASS | DONE (2026-03-11) | All 4 scenarios below |
| Phase 4: cross-repo UI smoke | deferred to final acceptance | see notes |

## Phase 3 verification — explicit-socket scenarios (2026-03-11)

| Scenario | Result |
|----------|--------|
| `explicit-tmux-socket-app-child-late-server.sh` | **PASS** — daemon inventories late-started pane under app-like normalized env |
| `explicit-tmux-socket-sanitized-path.sh` | **PASS** — inventory survives stripped PATH |
| `explicit-tmux-socket-shell-child-promotion.sh` | **PASS** |
| `explicit-tmux-socket-codex-midflight-proof.sh` | **PASS** — mid-flight: `provider=codex presence=managed`, v3 `thread.lifecycle=active execution=tool_running freshness=fresh/fresh/fresh`; post-completion: `presence=unmanaged provider=null` ✓ |

## Producer-side sub-fix summary (all DONE)

| Sub-task | Fix | Status |
|----------|-----|--------|
| T-XTERM-A6b | system binary resolver — shared `ps`/`lsof` with standard fallback PATH | DONE |
| T-XTERM-A6c | exact-socket shell inventory stays `presence=unmanaged` before Codex launch | DONE |
| T-XTERM-A6d | exact-socket Codex mid-flight v3 truth confirmed in repo-owned scenario | DONE |
| T-XTERM-A6e | Codex node-runtime discovery without direct process hint | DONE |
| T-XTERM-A6f | linked-session v3 row identity preserved (full exact location key) | DONE |
| T-XTERM-A6g | `ui.changes.v3` emits `remove(old) + upsert(new)` on exact-identity churn | DONE |
| T-XTERM-A6h | Codex JSONL HOME fallback resolves to `~/.codex` | DONE |

## Phase 4 notes (cross-repo UI smoke)

Phase 4 requires `agtmux-term` metadata-enabled XCUITest to pass. Given:
- All producer-side fixes are landed and verified
- Phase 3 explicit-socket scenarios demonstrate end-to-end daemon correctness
- agtmux-term side is actively migrating to sync-v3 (Codex in %2 is working)

Phase 4 is deferred to final acceptance after agtmux-term v3 migration completes. T-XTERM-A6 producer gate is satisfied.

## Verification evidence

- `just verify` → PASS (213 tests, 0 failed) — blockers T-XTERM-A5, T-VERIFY-FIX cleared
- `cargo fmt --check` PASS
- `cargo clippy --workspace -- -D warnings` PASS
- 4/4 explicit-socket e2e scenarios PASS

## Review Verdict — GO

- Root cause (delimiter bug) fixed in Phase 2 (already landed)
- All sub-fixes (A6b–A6h) confirmed DONE
- Phase 3 explicit-socket scenarios all PASS (2026-03-11)
- Phase 4 (cross-repo UI smoke) deferred pending agtmux-term v3 migration, but producer side is fully verified
- T-XTERM-A6 is ready to be marked DONE (producer scope)
