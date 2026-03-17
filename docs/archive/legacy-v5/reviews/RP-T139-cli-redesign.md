# Review Pack — T-139 CLI 全体再設計

## Objective
- Task: T-139 (a/b/c/d)
- Goal: `list-panes`/`list-windows`/`list-sessions`/`status`/`tmux-status` を全廃し、
  新 CLI (`ls` / `pick` / `watch` / `wait` / `json` / `bar`) を実装する

## Summary

Phase 6 Wave 3 として CLI を全面再設計（後方互換不要）。

- **T-139a**: コマンド骨格 + `ls` (tree/session/pane) + `bar`
  - `context.rs` (short_path, git_branch_for_path, consensus_str, build_branch_map)
  - `cmd_ls.rs` (format_ls_tree / format_ls_session / format_ls_pane)
  - `client.rs` 大幅改修（旧コマンド群削除、`cmd_bar`/`format_bar` 新設）
  - `server.rs`: `"git_branch": null` placeholder 追加
  - `cli.rs`: コマンド定義全面改訂（deprecated コマンド除去）
  - `main.rs`: bare `agtmux` → `Ls(default)` にルーティング

- **T-139b**: `agtmux pick` — fzf 統合 + `tmux switch-client`
- **T-139c**: `agtmux watch` — ANSI フルスクリーン更新ループ（crossterm 不使用）
- **T-139d**: `agtmux wait` — exit code 4-way / `agtmux json` — schema v1

## Change scope

| File | 変更種別 |
|------|---------|
| `crates/agtmux-runtime/src/context.rs` | 新規 |
| `crates/agtmux-runtime/src/cmd_ls.rs` | 新規 |
| `crates/agtmux-runtime/src/cmd_pick.rs` | 新規 |
| `crates/agtmux-runtime/src/cmd_watch.rs` | 新規 |
| `crates/agtmux-runtime/src/cmd_wait.rs` | 新規 |
| `crates/agtmux-runtime/src/cmd_json.rs` | 新規 |
| `crates/agtmux-runtime/src/client.rs` | 大幅改修（旧コマンド群削除） |
| `crates/agtmux-runtime/src/cli.rs` | 全面改訂 |
| `crates/agtmux-runtime/src/main.rs` | ルーティング更新 |
| `crates/agtmux-runtime/src/server.rs` | `"git_branch": null` 追加 |

## Verification evidence

- `just verify` (fmt + lint + test): **751 tests PASS**
  - T-139a 完了時点: 711 → 724 (+13)
  - T-139b/c/d 完了時点: 724 → 751 (+27)
  - 追加テスト内訳: context 11, cmd_ls 24, client(bar) 6, cmd_pick 3, cmd_watch 2, cmd_wait 8, cmd_json 14 = 68 追加; 旧テスト ~28 削除; 純増 +40

## Risk declaration

- **Breaking change**: yes — 旧コマンド (`list-panes`, `list-windows`, `list-sessions`, `status`, `tmux-status`) は削除
  - 現ユーザー: 設計者本人のみ。外部ユーザー・CI 参照なし。ドキュメント上も後方互換不要を明記
- **Fallbacks**: none — 意図的 (Fail loudly policy)
- **Known gaps / follow-ups**:
  - `server.rs` の `"git_branch": null` は client-side resolution のプレースホルダー。client が CWD ごとに `git rev-parse` を同期呼び出しする（性能は許容範囲だが将来 async 化候補）
  - `agtmux watch` は crossterm を使わない ANSI 簡易クリアのため、リサイズ対応なし（ターミナルリサイズで崩れる可能性あり）
  - T-135b (Claude JSONL conversation title 抽出) は別タスクで未着手

## Reviewer request

以下を確認してください。

1. **`cmd_wait.rs` exit code 契約**: `--idle` / `--no-waiting` 未指定時の挙動（現: どちらも指定しなければ即 0 返却）が意図通りか
2. **`agtmux pick` fzf 統合**: fzf 未インストール時のエラーメッセージが適切か
3. **`format_ls_tree` の空セッション / 空ウィンドウ処理**: pane 数 0 の session や window_name が空文字列のときに表示が崩れないか
4. **`git_branch_for_path` の同期ブロック**: 多数の unique CWD がある場合のパフォーマンス影響
5. **後方互換削除 (`list-panes` 等)**: 既存スクリプト (`scripts/tests/e2e/contract/run-all.sh` 等) への影響がないか確認

Verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO

---

## Review Result (2026-02-28)

**判定: GO_WITH_CONDITIONS**

### Blocking Issues

**B-1** (HIGH): E2E コントラクトスクリプトが廃止コマンドを呼び出している
- `scripts/tests/e2e/contract/test-schema.sh:20` — `list-panes --json`
- `scripts/tests/e2e/contract/test-waiting-states.sh:62,70,96,102` — `list-windows`, `list-sessions`
- `scripts/tests/e2e/contract/test-error-state.sh:54` — `list-windows`
- `scripts/tests/e2e/contract/test-list-consistency.sh:66,74,91` — `list-panes --json`, `list-sessions`, `list-windows`
- `scripts/tests/e2e/contract/test-multi-pane.sh:87` — `list-sessions`
- `scripts/tests/e2e/harness/common.sh:65,74` — `list-panes --json`
- Fix: **T-140 として `docs/60_tasks.md` に follow-up task 登録済み** → GO 許可条件を満たす

### Non-Blocking Issues

- **N-1**: `agtmux wait` 両フラグ未指定時は暗黙に `--idle` 動作 (`main.rs:63`) — help text と整合、許容
- **N-2**: `git_branch_for_path` が watch ループ内で毎イテレーション同期呼び出し (`cmd_watch.rs:20`) — 将来の async 化候補
- **N-3**: `which fzf` 使用 (`cmd_pick.rs:170`) — `command -v` の方が堅牢だが軽微
- **N-4**: `format_ls_tree` の空ウィンドウ・タイトル欠落は `"(unnamed)"` フォールバックで正しく処理済み

### 欠落テスト

- `agtmux wait --idle --no-waiting` 両方同時指定ケース（`main.rs:63` は `no_waiting` 優先だが相互排他未強制）
- `cmd_watch.rs` は format 統合テストなし（許容範囲）

### Orchestrator GO 決定

T-140 が `docs/60_tasks.md` に登録されたため、GO_WITH_CONDITIONS の条件を満たす。
**Orchestrator: GO — commit 許可**
