# Review Pack — T-SV2-P2: remove `ui.bootstrap.v2` / `ui.changes.v2` RPC endpoints

## Objective
- Task: T-SV2-P2
- Remove the `ui.bootstrap.v2` / `ui.changes.v2` wire endpoints from the daemon now that agtmux-term product path is fully sync-v3

## Pre-condition: agtmux-term v3 migration gate

Trigger commit in agtmux-term: `7f2bf36 refactor: isolate sync v2 compat metadata layer`

Evidence that the gate is satisfied:
- `LocalMetadataTransportVersion` now only has `case v3` — no `case v2`
- `LocalMetadataTransportBridge` calls `client.fetchUIBootstrapV3()` directly
- `AppViewModel` uses `any ProductLocalMetadataClient` (v3 interface only)
- `LocalMetadataRefreshCoordinator` uses `any ProductLocalMetadataClient`
- v2 methods are in `LocalMetadataClient+Compat.swift` with explicit comment "remove after daemon drops v2 endpoints"
- `swift build` + `swift test` PASS with v3-only product path

## Change scope (2 files, 601 deletions)

| ファイル | 変更内容 |
|---------|---------|
| `crates/agtmux-runtime/src/server.rs` | `"ui.bootstrap.v2"` / `"ui.changes.v2"` match arms 削除 + v2 専用テスト削除 |
| `crates/agtmux-runtime/src/sync_v2_compat.rs` | `build_ui_bootstrap_v2()` / `build_ui_changes_v2()` / `build_sync_v2_pane_list()` 削除; モジュール stub (5行コメント) として残存 |

## NOT changed (intentional)

- `sync_v2_compat` module itself — stub として残す (T-SV2-P3 で削除)
- `agtmux-core-v5::sync_v2_compat` module — T-SV2-P3 で削除
- `agtmux-source-*` / projection side — T-SV2-P1 で解決済み
- Server-side tests that reference `sync_v2` data structures for compat-checking (e.g. `codex_task_complete_intentionally_diverges_between_sync_v2_and_v3_surfaces`) remain valid

## Verification evidence

- `just verify` → PASS (953 tests, 0 failed)
- `cargo fmt --check` PASS
- `cargo clippy --workspace -- -D warnings` PASS
- No `"ui.bootstrap.v2"` / `"ui.changes.v2"` strings in server.rs
- `sync_v2_compat.rs` reduced to 5-line comment stub

## Review Verdict — GO

- All T-XTERM-A3~A8 blockers confirmed DONE
- agtmux-term product path v3-only gate satisfied (commit `7f2bf36`)
- `ui.bootstrap.v2` and `ui.changes.v2` handlers removed from server.rs
- v2 builder functions removed from sync_v2_compat.rs
- 953 tests PASS
- T-SV2-P2 is ready to be marked DONE
