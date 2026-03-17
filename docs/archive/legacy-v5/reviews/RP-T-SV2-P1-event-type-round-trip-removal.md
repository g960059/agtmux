# Review Pack — T-SV2-P1: sync-v2 event_type string round-trip removal

## Objective
- Task: T-SV2-P1
- Remove the `ActivityState → "activity.running" string → ActivityState` round-trip from the source adapter pipeline

## Summary

`SourceEventV2.event_type: String` を `activity_state: ActivityState` に置き換え、source adapters が直接型付きフィールドを設定するようにした。

### Before (round-trip)
```
Source → sync_v2_compat::activity_event_type(state) → "activity.running"
       → SourceEventV2 { event_type: "activity.running" }
       → projection: parse_activity_state("activity.running") → ActivityState::Running
```

### After (direct)
```
Source → SourceEventV2 { activity_state: ActivityState::Running }
       → projection: event.activity_state (直接参照)
```

## Change scope (17 files)

| カテゴリ | ファイル |
|---------|---------|
| Core type | `agtmux-core-v5/src/types.rs` — `event_type: String` → `activity_state: ActivityState` |
| Projection | `agtmux-daemon-v5/src/projection.rs` — `parse_activity_state()` → `event.activity_state` 直接参照 |
| Source adapters | `agtmux-source-poller/src/source.rs`, `agtmux-source-claude-hooks/src/translate.rs`, `agtmux-source-claude-jsonl/src/source.rs`, `agtmux-source-codex-jsonl/src/translate.rs` |
| V3 normalizers | `agtmux-daemon-v5/src/claude_v3.rs`, `agtmux-daemon-v5/src/codex_v3.rs` |
| Runtime | `agtmux-runtime/src/poll_loop.rs`, `agtmux-runtime/src/server.rs`, `agtmux-runtime/src/sync_v3_runtime.rs` |
| Gateway | `agtmux-gateway/src/gateway.rs` |
| Resolver | `agtmux-core-v5/src/resolver.rs` |
| Appserver compat | `agtmux-source-codex-appserver/src/translate.rs` — raw string → `ActivityState` の正規化1箇所 |

## NOT changed (intentional)

- `sync_v2_compat` module 自体 (Phase 3 で削除)
- `ui.bootstrap.v2` / `ui.changes.v2` endpoints (Phase 2)
- wire protocol / 外部向け JSON 形式

## Verification evidence

- `just verify` → PASS (967 tests, 0 failed)
- `cargo fmt --check` PASS
- `cargo clippy --workspace -- -D warnings` PASS

## Review Verdict — GO

- `event_type: String` の全使用箇所が `activity_state: ActivityState` に移行済み
- `sync_v2_compat::activity_event_type()` / `parse_activity_state()` の呼び出しがソースアダプターから除去済み
- projection.rs は `event.activity_state` を直接参照
- 967 tests PASS
