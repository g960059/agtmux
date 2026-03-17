# Review Pack — T-VERIFY-FIX: clippy 修正 + just verify green 化

## Objective
- Task: T-VERIFY-FIX
- Acceptance criteria: `just verify` PASS

## Summary
`d2ddf0f` (codex exec spool) で導入されたコードが clippy lint をパスしていなかった。
4 ファイルを修正して `just verify` を green に戻した。
主な変更は機械的な lint 修正 3 件と、過剰な guard 条件削除 1 件（behavior change あり）。

## Change scope (4 files)

| ファイル | 変更内容 | 種別 |
|--------|---------|------|
| `crates/agtmux-source-codex-jsonl/src/discovery.rs:74` | nested `if let` → `let ... else` で平坦化（`collapsible_if` 解消） | lint fix |
| `crates/agtmux-source-codex-jsonl/src/translate.rs:14` | `#[allow(clippy::too_many_arguments)]` 追加 | lint fix |
| `crates/agtmux-runtime/src/sync_v3_runtime.rs` | `TimeZone` import をテスト専用スコープに移動 | lint fix |
| `crates/agtmux-runtime/src/poll_loop.rs:725` | `&& metadata_failure_reason.is_none()` 条件を削除 | behavior change |

### `poll_loop.rs` 変更の詳細
変更前:
```rust
if !metadata_backoff_active && metadata_failure_reason.is_none() {
    // Claude JSONL discovery
```
変更後:
```rust
if !metadata_backoff_active {
    // Claude JSONL discovery
```
意図: process metadata が失敗しても Claude JSONL discovery を継続して実行する。
この変更で `poll_tick_idle_claude_restart_bootstrap_surfaces_sync_v3_idle_truth` テストが復旧。
daemon 再起動時に process metadata が一時的に失敗しても Claude JSONL discovery がスキップされなくなる。

## Verification evidence

- `just verify` (fmt + clippy + test) → **PASS** (211 agtmux + all workspace tests)
- 変更前は clippy 2 件 + unit test 1 件が失敗していた

## Risk declaration
- Breaking change: no（外部 API/プロトコル変更なし）
- `poll_loop.rs` の behavior change:
  - リスク: process metadata 失敗時に Claude JSONL が意図せず動作する可能性（ただし既存テストはすべて PASS）
  - 軽減: `metadata_backoff_active` ガードは残っており、完全無制限ではない
  - `metadata_failure_reason` は元々 Claude JSONL discovery を止める意図があったかが不明
- Known gaps:
  - `metadata_failure_reason` を `is_none()` でゲートしていた元の意図のドキュメントが不明

## Review Verdict — NO_GO (poll_loop.rs)

Codex review 結果 (2026-03-10):

**Blocking (P2)**:
- `poll_loop.rs:725` の `metadata_failure_reason.is_none()` 削除は NO_GO
- 理由: process scan が timeout/失敗/空のとき `to_pane_snapshot()` は shallow fallback になり、neutral `node`/`python` pane が `process_hint=None` で届く。guard を外すと Step 6b がそれらに CWD ベース discovery を実行し、古い `~/.claude/projects/<cwd>/*.jsonl` から bootstrap を emit して unrelated runtime を managed Claude と誤帰属しうる
- 元の guard は fail-closed behavior の核で、単純削除はそれを後退させる
- テスト PASS は FakeTmuxBackend が実 scan と異なる挙動をしているための見た目のみ

**Non-blocking**:
- discovery.rs / translate.rs / sync_v3_runtime.rs の 3 件は機械的 lint 修正で問題なし

## T-VERIFY-FIX-B Resolution — GO

Narrower fix implemented (2026-03-10):

**Root cause**: `scan_all_processes()` fails in this env with `ps: operation not permitted`, setting `metadata_failure_reason`. The original broad gate `metadata_failure_reason.is_none()` also excluded explicit `process_hint=Some("claude")` panes, breaking the restart bootstrap test.

**Fix**: Extracted `is_claude_jsonl_candidate(process_hint, cmd, metadata_available)`:
- `Some("claude")` → always discoverable (even during metadata degradation)
- `process_hint=None` neutral → only when `metadata_available=true` (fail-closed preserved)

**Added regression test**: `claude_jsonl_candidates_require_metadata_for_neutral_runtimes`

**Gate**: `just verify` PASS (212 tests) ✅

**Final verdict: GO**
