# Review Pack — T-SV2-P3: delete `agtmux-core-v5::sync_v2_compat` module

## Objective
- Task: T-SV2-P3
- Delete the `sync_v2_compat` module from `agtmux-core-v5` and remove all references, now that T-SV2-P1 + T-SV2-P2 are DONE

## Change scope (7 files, 317 deletions)

| ファイル | 変更内容 |
|---------|---------|
| `crates/agtmux-core-v5/src/sync_v2_compat.rs` | **削除** — `activity_event_type()` / `parse_activity_state()` / consts 全除去 |
| `crates/agtmux-core-v5/src/lib.rs` | `pub mod sync_v2_compat;` を削除 |
| `crates/agtmux-runtime/src/sync_v2_compat.rs` | **削除** — T-SV2-P2 で残した 5行 stub も除去 |
| `crates/agtmux-runtime/src/main.rs` | sync_v2_compat import 削除 |
| `crates/agtmux-source-codex-appserver/src/translate.rs` | `sync_v2_compat::parse_activity_state()` を inline match に置き換え |
| `crates/agtmux-daemon-v5/src/projection.rs` | `use sync_v2_compat` 削除; `make_event()` を `ActivityState` 直受けに変更; `legacy_activity_state()` ローカルヘルパー追加; 19 call sites 更新 |
| `crates/agtmux-daemon-v5/src/codex_v3.rs` | 参照クリーンアップ |

## Strategy for projection.rs

`make_event()` test helper signature changed from `event_type: &str` to `activity_state: ActivityState`. Added `legacy_activity_state(event_type: &str) -> ActivityState` as a test-local helper that inlines the mapping. All 19 direct `make_event()` call sites updated to use `legacy_activity_state("string")`.

Wrapper helpers (`det_event`, `heur_event`, `codex_poller_event`, `claude_poller_event`) already use `legacy_activity_state(event_type)` in their body.

## NOT changed (intentional)

- `agtmux-runtime/src/sync_v2_compat.rs` was already a 5-line stub from T-SV2-P2; fully removed now
- Wire protocol / external JSON format unchanged
- No behavior change — T-SV2-P1/P2 already removed all product-path usage

## Verification evidence

- `just verify` → PASS (948 tests, 0 failed)
- `cargo fmt --check` PASS
- `cargo clippy --workspace -- -D warnings` PASS
- No `sync_v2_compat` references anywhere in workspace (except the now-deleted files)

## Review Verdict — GO

- Both blockers T-SV2-P1 (DONE) + T-SV2-P2 (DONE) confirmed
- `agtmux-core-v5::sync_v2_compat` module fully deleted
- `agtmux-runtime::sync_v2_compat` stub deleted
- All references updated with test-local helpers where needed
- 948 tests PASS
- T-SV2-P3 is ready to be marked DONE
