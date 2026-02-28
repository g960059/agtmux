# Review Pack — T-135b Claude JSONL Conversation Title Extraction

## Objective
- Task: T-135b
- Goal: Claude Code が書き込む `custom-title` JSONL イベントから会話タイトルを抽出し、
  `DaemonState.conversation_titles` に格納して `agtmux json` / `agtmux ls` で表示する

## Summary

T-135a (Codex) は `thread/list` API から会話タイトルを取得する仕組みを実装済み。
T-135b では Claude JSONL の同等機能を実装する。

Claude Code は JSONL ファイルにタイトル変更のたびに以下のイベントを追記する:
```json
{"type": "custom-title", "customTitle": "会話タイトル", "sessionId": "uuid"}
```
最後に出現した `customTitle` が現在のタイトル。

実装方針: JSONL watcher がタイトルを蓄積 → poll_loop が discovery 走査で `conversation_titles` に挿入。
`server.rs` の `build_pane_list` は T-135a 実装時に `conversation_titles.get(session_key)` 参照済みのため変更不要。

## Change scope

| File | 変更種別 |
|------|---------|
| `crates/agtmux-source-claude-jsonl/src/translate.rs` | `ClaudeJsonlLine.custom_title` 追加、`timestamp` を `Option<>` 化 |
| `crates/agtmux-source-claude-jsonl/src/watcher.rs` | `SessionFileWatcher.last_title` + `last_title()`/`set_title()` 追加 |
| `crates/agtmux-source-claude-jsonl/src/source.rs` | `poll_files()` で `custom-title` 行を検出・転送 |
| `crates/agtmux-runtime/src/poll_loop.rs` | `poll_files()` 直後に `conversation_titles` へ挿入 |

## Verification evidence

- `just verify` => **753 tests PASS** (751 → 753, +2 新規テスト)
  - `custom_title_field_deserialized_from_custom_title_line` (translate.rs)
  - `poll_files_extracts_custom_title_from_jsonl` (source.rs)

## Risk declaration

- **Breaking change**: no — `conversation_title` フィールドは既存 API に追加済み（T-130）
- **Fallbacks**: `last_title = None` → `conversation_title: null` (変化なし、既存挙動と同じ)
- **Known gaps / follow-ups**:
  - `timestamp` を `Option<>` 化したことによる既存テストへの影響 (subagent が修正済み)
  - pane が消えてウォッチャー削除後に再出現した場合、次回 `custom-title` まで null (仕様上許容)

## Reviewer request

以下を確認してください:

1. **`timestamp` の `Option<>` 化**: `custom-title` 行に timestamp フィールドがないため `Option<DateTime<Utc>>` に変更。
   `translate()` が `unwrap_or_else(Utc::now)` で補完しているが、既存 activity event への影響がないか
2. **borrow checker workaround**: `poll_loop.rs` で `Vec` に一旦収集してから `insert` する実装の正しさ
3. **テストカバレッジ**: 空文字列タイトルのスキップ、複数回 custom-title の最後の値採用、が十分テストされているか
4. **`server.rs` 変更不要の前提**: `build_pane_list` が `state.conversation_titles.get(&pane.session_key)` で Claude session_id を lookup できるか

Verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO

---

## Review Result (2026-02-28)

**判定: GO_WITH_CONDITIONS → Orchestrator GO**

### Reviewer 1 (codex-style) — GO_WITH_CONDITIONS

**条件 (両方とも修正済み)**:
- C-1: `poll_loop.rs` コメント「`sessions-index.json`」→「`custom-title JSONL events`」修正 ✅
- C-2: `source.rs` 空文字列スキップのテスト `poll_files_ignores_empty_custom_title` 追加 ✅

**Non-blocking**:
- `unwrap_or_else(Utc::now)` は現時点では発動経路なし（custom-title は continue で除外）
- Vec 収集 → insert パターンは borrow checker 制約の正しい workaround
- watcher 差し替え時の stale title は rotation 後の新規 custom-title で上書き

### Reviewer 2 (Claude) — GO

- Blocking issues なし
- bootstrap-on-custom-title-only tick の副作用は既存挙動と同じで許容
- `disc.session_id`, `disc.pane_id` フィールド名確認済み

### Final: GO

条件修正完了後 `just verify` 754 tests PASS (751→753→754, +3 new tests)。
**Orchestrator: GO — commit 許可**
