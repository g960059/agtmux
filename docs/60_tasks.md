# Tasks Board (source of truth for execution; Orchestrator only)

## Status
- TODO / DOING / REVIEW / DONE / BLOCKED

## Rules
- Task IDs are stable. If splitting, use suffix (`T-010a`, `T-010b`).
- Every TODO task must declare `blocked_by`.
- REVIEW には Review Pack (`docs/85_reviews/RP-...`) を添付する。
- DONE は証跡（`just` 実行結果/test/review、必要時のみ commit/PR）を短く残す。
- online/e2e source test（codex/claude）を走らせる前に `just preflight-online` を必須実行する。

## TODO

### Phase 7 — E2E テスト本格導入

- [x] T-140 (P2) E2E コントラクトスクリプト CLI 移行 — DONE (2026-02-28)
  - T-139 で廃止されたコマンド群を新 CLI に置き換え（follow-up from T-139 review B-1）
  - 変更ファイル 9 件:
    - `harness/common.sh`: `jq_get` / debug → `agtmux json`, `.panes[]` jq path
    - `test-schema.sh`: `agtmux json` schema v1、object/array 検証に変更
    - `test-waiting-states.sh`: `list-windows` → `agtmux ls`、`list-sessions` → `agtmux ls --group=session`、activity_state 期待値 → snake_case
    - `test-error-state.sh`: `list-windows` → `agtmux ls`、activity_state → snake_case
    - `test-list-consistency.sh`: `list-panes --json` → `json`、`list-sessions`/`list-windows` → `agtmux ls`、jq filter → snake_case
    - `test-multi-pane.sh`: `list-sessions` → `agtmux ls --group=session`、activity_state → snake_case
    - `test-freshness-fallback.sh`: `activity_state: "running"`（snake_case）
    - `test-claude-state.sh` / `test-codex-state.sh`: `activity_state` → snake_case
  - Gate: `bash -n` syntax check PASS (10 scripts); `just verify` 751 tests PASS

- [x] T-137 (P2) Layer 2 Contract E2E 基盤 — DONE (2026-02-28)
  - `scripts/tests/e2e/harness/{common,daemon,inject}.sh`
  - `scripts/tests/e2e/contract/test-schema.sh`, `test-claude-state.sh`, `test-codex-state.sh`, `test-waiting-states.sh`, `test-list-consistency.sh`, `test-multi-pane.sh`, `run-all.sh`
  - justfile `preflight-contract` / `e2e-contract` targets
  - Gate: `just e2e-contract` 6 passed, 0 failed

- [x] T-138 (P3) Layer 3 Provider-Adapter Detection E2E — DONE (2026-02-28)
  - `providers/claude/adapter.sh`, `providers/codex/adapter.sh`, `providers/gemini/adapter.sh.stub`
  - `scenarios/single-agent-lifecycle.sh`, `multi-agent-same-session.sh`, `same-cwd-multi-pane.sh`, `provider-switch.sh`
  - `online/run-all.sh` (PROVIDER= env var, E2E_SKIP_SCENARIOS support)
  - justfile: `e2e-online`, `e2e-online-claude`, `e2e-online-codex` targets 追加済み
  - Gate: syntax check PASS; live CLI run requires `just preflight-online`

### Phase 6 Wave 2 — CLI 表示リデザイン

- [x] T-135b (P4) Claude JSONL conversation title 抽出 — DONE (2026-02-28)
  - Source: JSONL `{"type":"custom-title","customTitle":"...","sessionId":"..."}` イベント
  - 最後に出現した `customTitle` が現在のタイトル（セッション中に複数回出現しうる）
  - 変更ファイル 4 件:
    - `translate.rs`: `ClaudeJsonlLine` に `custom_title: Option<String>` フィールド追加
    - `watcher.rs`: `SessionFileWatcher` に `last_title: Option<String>` + `last_title()`/`set_title()` メソッド追加
    - `source.rs`: `poll_files()` ループ内で `custom-title` 行を検出し `watcher.set_title()` を呼ぶ
    - `poll_loop.rs`: `poll_files()` 直後に discoveries を走査し `st.conversation_titles[session_id] = title`
  - 新規テスト 2 件 → 753 tests expected
  - blocked_by: T-135a (DONE)

- [x] T-135c (P4) Claude summary + sessions-index.json フォールバック — DONE (2026-03-01)
  - Priority chain: `custom-title > summary(watcher) > summary(sessions-index) > firstPrompt(sessions-index)`
  - 変更ファイル 7 件:
    - `translate.rs`: `ClaudeJsonlLine` に `summary: Option<String>` フィールド追加
    - `watcher.rs`: `last_summary: Option<String>` + `last_summary()`/`set_summary()` メソッド追加
    - `source.rs`: `type=summary` 行を検出し `watcher.set_summary()` を呼ぶ（2 新規テスト）
    - `discovery.rs`: `SessionIndexEntry` に `summary`/`first_prompt` フィールド追加 + `read_session_index_entry()` pub fn（2 新規テスト）
    - `poll_loop.rs`: 3段階優先チェーン実装（summary→custom-title→sessions-index fallback）
    - `lib.rs`: `pub use discovery::read_session_index_entry` エクスポート追加
    - `scenarios/claude-summary.sh`: 新規 e2e テスト（Phase 3: summary→title, Phase 4: custom-title wins）
    - `online/run-all.sh`: `claude-title` + `claude-summary` + `codex-title` シナリオ追加
  - 新規テスト 6 件（unit）+ 1 件（e2e）
  - Gate: `just verify` PASS; e2e PROVIDER=claude 5 passed, 0 failed
  - blocked_by: T-135b (DONE)

### Phase 6 Wave 3 — CLI 全体再設計（T-139 拡張）

T-139〜T-142 を統合し CLI を全面再設計（後方互換不要）。
設計概要: `.claude/plans/gleaming-prancing-wilkes.md` 参照（実装承認済み 2026-02-28）。

- [x] T-139a (P2) CLI Core — コマンド骨格 + `ls` + triage — DONE (2026-02-28)
  - `cli.rs`: 全コマンド再定義（廃止: list-panes/list-windows/list-sessions/tmux-status/status）
  - `context.rs`: `short_path`, `git_branch_for_path`, `truncate_branch`, `consensus_str`, `build_branch_map`, `relative_time`, `resolve_color`, `provider_short` (新規)
  - `cmd_ls.rs`: `format_ls_tree` / `format_ls_session` / `format_ls_pane` + `cmd_ls` (新規)
  - `client.rs`: 旧 format 関数削除、新 `cmd_bar` / `format_bar`（`--tmux` フラグ対応）
  - `server.rs`: `"git_branch": null` フィールド追加（client-side branch resolution を選択）
  - `main.rs`: ルーティング更新、bare `agtmux` → `Ls(default)`
  - 新規テスト 41 件、旧テスト ~28 件削除、净増 +13、724 tests total
  - Gate: `just verify` 724 tests PASS

- [x] T-139b (P3) Navigation — pick — DONE (2026-02-28)
  - `cmd_pick.rs`: `format_pick_candidates`, `cmd_pick` — fzf 統合 + tmux switch-client
  - `--dry-run`: 候補一覧表示のみ / `--waiting`: Waiting pane のみフィルタ
  - 3 new tests
  - Gate: `just verify` 724 → 727 tests (counted in T-139b/c/d 合計)

- [x] T-139c (P3) Monitor — watch + bar — DONE (2026-02-28)
  - `cmd_watch.rs`: ANSI クリア (`\x1b[2J\x1b[H`) + `format_ls_tree` ループ + Ctrl-C 終了
  - `--interval N`: 更新間隔（秒）; crossterm 不使用（依存追加なし）
  - 2 new tests
  - Gate: `just verify` PASS (T-139b/c/d 合計で確認)

- [x] T-139d (P3) Script — wait + json — DONE (2026-02-28)
  - `cmd_wait.rs`: `WaitCondition { Idle, NoWaiting }`, exit code 0/1/2/3, `\r` progress 表示
  - `cmd_json.rs`: schema v1 `{version:1, panes:[...]}`, normalize helpers, `--health`
  - 8 + 14 = 22 new tests
  - Gate: `just verify` 724 → 751 tests PASS (+27 net for T-139b/c/d)

## TODO

### Phase 8 — Sources Enhancement

OSC シーケンス調査（2026-03-01）と外部レポート評価に基づき合意した改善方針。
方針詳細: `docs/70_progress.md` (2026-03-01 エントリ)、`docs/80_decisions/ADR-20260301-osc-architecture.md`。
Phase 7 (Distribution) と独立して実施可能。

- [x] T-E01 (P1) Hooks coverage expansion — DONE (2026-03-01)
  - `translate.rs`: 6 new hook type mappings (SessionStart/SessionEnd/PermissionRequest/UserPromptSubmit/PostToolUseFailure/PreCompact)
  - `setup_hooks.rs`: HOOK_TYPES 5→11
  - `poll_loop.rs`: transcript_path_hints in DaemonState, Step 8b scan + P1 overlay in Step 6b
  - `discovery.rs`: `discovery_from_transcript_path()` public helper
  - Gate: `just verify` PASS (760+ tests)

- [x] T-E02 (P2) JSONL fd-based discovery — DONE (2026-03-01)
  - `discovery.rs`: `PaneDiscoveryHint` struct, `discover_jsonl_via_lsof()`, updated `discover_sessions()` signature
  - `source.rs`: updated call site
  - `poll_loop.rs`: Step 6b uses `Vec<PaneDiscoveryHint>` with pane_pid
  - Priority chain: P1 (transcript_path) > P2 (lsof) > P3 (CWD)
  - Gate: `just verify` PASS

- [x] T-E03 (P2) setup-hooks coverage verification + check subcommand — DONE (2026-03-01)
  - `cli.rs`: `--check` flag on SetupHooksOpts
  - `setup_hooks.rs`: `HookStatus`, `HookCheckResult`, `check_hooks()`
  - `main.rs`: branch on `opts.check`
  - Gate: `just verify` PASS

- [x] T-E08 (P1) `apply_hooks()` surgical merge — DONE (2026-03-03)
  - `merge_hooks_into_settings()` で per-type retain+push。他ツールのhooksを保持
  - commit: adbabce

- [x] T-E09 (P1) `agtmux setup-hooks --unregister` — DONE (2026-03-03)
  - `remove_hooks()` + `--unregister` フラグ。HOOK_TYPES 限定の surgical 削除
  - commit: adbabce

- [ ] T-term01 (P2) agtmux-term hooks 統合 — 申し送り資料に基づく実装
  - **目的**: agtmux-term 起動時の hooks 自動チェック + Register/Unregister UI
  - **申し送り**: `/tmp/agtmux-term-hooks-handoff.md`（永続化先: `docs/85_reviews/` か agtmux-term の docs へ）
  - **変更対象**: `agtmux-term` リポジトリ
    - `AppViewModel.swift`: `hookSetupStatus: @Published` + `performStartupHookCheck() async`
    - `SidebarView.swift`: ⚠ バッジ（isOffline パターン流用）
    - 空状態プロンプト: [Register Hooks] ボタン
    - Settings パネル: [Verify] [Re-register] [Unregister] ボタン
  - blocked_by: T-E08, T-E09

- [ ] T-E03a (P3) check_hooks() integration test — follow-up from RP review
  - **目的**: `check_hooks()` を temp settings.json に対して呼び出す integration test (partially registered case)
  - **変更対象**: `crates/agtmux-runtime/src/setup_hooks.rs` test module
  - blocked_by: T-E03

- [ ] T-E03b (P3) poll_loop P1 transcript_path hint integration test — follow-up from RP review
  - **目的**: SessionStart hook event が transcript_path_hints を populate し、次 tick で JSONL discovery に使われることを確認する poll_loop integration test
  - **変更対象**: `crates/agtmux-runtime/src/poll_loop.rs` test module
  - blocked_by: T-E01

- [x] T-E05 (P2) `Notification` フック `notification_type` 解析 — DONE (2026-03-03)
  - `resolve_event_type()` wrapper 追加。idle_prompt→waiting_input, permission_prompt→waiting_approval
  - commit: 1955b28

- [x] T-E06 (P2) `PreToolUse`/`PostToolUse` → `activity.running` マッピング — DONE (2026-03-03)
  - `normalize_event_type()` に PreToolUse/PostToolUse arm 追加
  - commit: 1955b28

- [x] T-E07 (P2) ブレイルスピナー pane_title 検出（ポーラー拡張）— DONE (2026-03-03)
  - evidence.rs: Running パターンを完全 10 文字セットに拡張 (⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏)
  - source.rs: `classify_claude_title_activity()` + title-upgrade guard (Claude-only, Unknown/Idle のみ昇格)
  - commit: 1955b28

- [ ] T-E04 (P3) OSC Tap source — C-017 `agtmux-source-osc-tap` [Post-MVP]
  - **目的**: tmux `pipe-pane` 経由で OSC 9;4 progress bar シグナルを取得する semi-deterministic source
  - **前提条件**: tmux 3.3+、pipe-pane 先占競合なし（capability-gated）
  - **変更対象**:
    - 新規 crate `crates/agtmux-source-osc-tap/`
      - `OscTapManager`: pane 単位の pipe-pane 起動/停止/先占チェック
      - `OscParser`: OSC 9;4 バイトストリーム解析（BEL `\x07` / ST `\x1b\\` 両終端対応）
      - `OscTapSource`: SourceEventV2 への変換（confidence: 0.92）
    - `crates/agtmux-core-v5/src/types.rs`: `SourceKind::OscTap` 追加
    - `crates/agtmux-runtime/src/poll_loop.rs`: capability check → tap 起動/停止 wiring
    - source rank: `hooks (0) > jsonl (1) > osc_tap (2) > poller (3)`
  - **制約**:
    - OSC 133 は採用しない（Claude Code が emit しない、issue #26235 open）
    - OSC 不在は negative evidence に使用しない
    - pipe-pane 先占競合時は graceful skip して poller fallback
  - **Gate**: `just verify` PASS + OSC 9;4 シーケンスのユニットテスト PASS
  - blocked_by: T-E01, T-E02

### Phase 9 — Codex JSONL セマンティックソース（根本解決）

2026-03-02 の 4 エージェント研究チーム調査に基づき、mtime ベース検出を JSONL セマンティック解析に完全置換する。
設計詳細: `docs/research/20260302/05_synthesis.md`

**重要な発見（調査チーム確認済み）:**
- JSONL JSON キーは `.payload.type`（`.data.type` ではない）
- 正しいイベント名: `task_started`, `task_complete`, `entered_review_mode`, `exited_review_mode`
- keepalive 行は存在しない（Idle 時はファイル書き込みが停止するのみ）
- `entered_review_mode` が WaitingApproval の確定シグナル

- [x] T-codex01a (P1) Codex 検出 回帰テスト + e2e テスト作成 — DONE (2026-03-03)
  - **目的**: 現在のバグを再現するユニット/e2e テストを先に書く（TDD）
  - **回帰ケース（unit）**:
    - `scan_jsonl_sessions()` で古い日付ディレクトリのセッションが未検出になる
    - mtime ベースで task_started/task_complete を誤分類する
    - WaitingApproval が検出されない（entered_review_mode 未処理）
  - **e2e ケース**:
    - `scripts/tests/e2e/online/scenarios/codex-semantic-states.sh`
    - Phase 1: Codex 起動 → task_started 注入 → Running を確認
    - Phase 2: task_complete 注入 → WaitingInput を確認
    - Phase 3: entered_review_mode 注入 → WaitingApproval を確認
    - Phase 4: exited_review_mode + task_complete → WaitingInput を確認
  - **変更対象**: `crates/agtmux-source-codex-jsonl/src/` の各モジュールのテスト
  - blocked_by: なし

- [x] T-codex01b (P1) `agtmux-source-codex-jsonl` 新クレート実装 — DONE (2026-03-03)
  - **目的**: JSONL セマンティック解析で Codex 状態を正確に検出
  - **新規クレート**: `crates/agtmux-source-codex-jsonl/`
    - `discovery.rs`: `lsof -p <pid> -d cwd -Fn` → CWD 取得 → 全日付ディレクトリ走査（日付フィルタなし）
    - `watcher.rs`: inode + byte_offset tracking + partial_line_buf（claude-jsonl と同パターン）
    - `fsm.rs`: `Init → Running → ToolExecuting / WaitingApproval / WaitingInput → Ended` FSM
    - `translate.rs`: FSM state → `SourceEventV2`
    - `source.rs`: poll_loop ブリッジ
  - **FSM 遷移**:
    - `task_started` → Running
    - `function_call` → ToolExecuting、`function_call_output` → Running
    - `entered_review_mode` → WaitingApproval、`exited_review_mode` → Running
    - `task_complete` / `turn_aborted` → WaitingInput
    - WaitingInput + process exit or 600s → Ended
  - **Provider**: `Provider::Codex`, `SourceKind::CodexJsonl`, `EvidenceTier::Deterministic`
  - 50 unit tests, `SourceKind::CodexJsonl` added to core types
  - Gate: `just verify` 800+ tests PASS

- [x] T-codex01c (P1) poll_loop 接続 + 旧コード削除 — DONE (2026-03-03)
  - **poll_loop.rs**: Step 6a（App Server ブロック）+ Step 6a-bis（scan_jsonl_sessions）を新ソース呼び出しに差し替え
  - **削除対象** (`codex_poller.rs`): 700行→4行スタブに置換（`CodexAppServerClient`、`scan_jsonl_sessions`、Pass1/2/3、`CodexCaptureTracker`、全定数）
  - **DaemonState** から削除: `codex_appserver_client`, `codex_supervisor`, `codex_capture_tracker`, `codex_appserver_had_connection`
  - **DaemonState** に追加: `codex_jsonl_source: CodexJsonlSourceState`, `codex_jsonl_watchers`
  - `server.rs`: `codex_appserver` source kind 削除
  - Gateway sources: `CodexAppserver` → `CodexJsonl` に置換
  - Gate: `just verify` 800+ tests PASS, zero warnings

- [x] T-codex01d (P1) exec parity deterministic spool/hint — DONE (2026-03-10)
  - **目的**: `codex exec --json` / `codex --yolo` でも interactive transcript と同じ deterministic Codex semantic path へ入り、strict live proof で `primary=.running` を出せるようにする
  - **実装**:
    - `crates/agtmux-runtime/src/codex_exec_spool.rs` 新規: pane-bound NDJSON spool tracker、`session_meta` header、exec NDJSON → transcript-like JSONL 正規化
    - `poll_loop.rs`: joined capture を exact pane ごとに spool し、`CodexPaneHint.explicit_jsonl_path + session_key_override` で `CodexJsonl` discovery に直結
    - `discovery.rs`: explicit pane-bound JSONL binding を最優先し、missing explicit path では same-CWD fallback しない
    - `tmux capture-pane -J` helper 追加
  - **Gate**:
    - `cargo test -p agtmux codex_exec_spool -- --nocapture`
    - `cargo test -p agtmux poll_tick_exec_json_promotes_exact_pane_to_sync_v3_running_without_same_cwd_bleed -- --nocapture`
    - `cargo test -p agtmux poll_tick_exec_spool_rehydrates_running_truth_after_restart -- --nocapture`
    - `cargo test -p agtmux`

## DOING

### Clippy fix — just verify ブロッカー解消 ✅ DONE

- [x] T-VERIFY-FIX + T-VERIFY-FIX-B (P0) clippy 修正 + narrower poll_loop fix — DONE (2026-03-10)
  - **根本原因**: `d2ddf0f` コードが clippy をパスしていない + `scan_all_processes()` が `ps: operation not permitted` で `metadata_failure_reason` を立てた際、元の全体 guard が explicit `claude` pane まで除外する over-gating
  - **変更ファイル 5 件**:
    - `crates/agtmux-source-codex-jsonl/src/discovery.rs:74` — `let ... else` 平坦化 ✅
    - `crates/agtmux-source-codex-jsonl/src/translate.rs:14` — `#[allow(clippy::too_many_arguments)]` ✅
    - `crates/agtmux-runtime/src/sync_v3_runtime.rs` — `TimeZone` import をテスト専用スコープ ✅
    - `crates/agtmux-runtime/src/poll_loop.rs:319` — `is_claude_jsonl_candidate()` 追加 (narrower fix): `Some("claude")` は常に許可、`None` neutral は `metadata_failure_reason.is_none()` のときのみ
    - `crates/agtmux-runtime/src/poll_loop.rs:2113` — 回帰テスト `claude_jsonl_candidates_require_metadata_for_neutral_runtimes` 追加
  - **Gate**: `just verify` PASS (212 tests) ✅
  - RP: `docs/85_reviews/RP-T-VERIFY-FIX-clippy-green.md`

### Cross-repo agtmux-term compatibility recovery

- [x] T-XTERM-A3 (P0) Cross-repo: sync-v2 exact-identity handback — DONE (2026-03-11)
  - 目的: `build_sync_v2_pane_list()` が orphan managed panes（exact location null）を emit しないよう修正し、strict consumer が bootstrap を reject しないようにする
  - Phase 1+2 (2026-03-08): producer-side fix landed
    - `build_sync_v2_pane_list()` が `let Some(tmux_info) = state.last_panes.find(...)` else `continue` で unresolved panes を除外
    - regression `ui_bootstrap_v2_excludes_managed_pane_when_exact_location_is_unresolved` added
  - Phase 3 verification (2026-03-11):
    - `cargo test -p agtmux ui_bootstrap_v2_` → 7/7 PASS
    - `cargo build -p agtmux` → PASS (v0.1.17)
    - `swift test --filter AgtmuxSyncV2DecodingTests` → 10/10 PASS
    - `swift test --filter AppViewModelA0Tests/testLiveMarch8BootstrapSampleWithNullExactLocationFieldsFailsClosedAndSurfacesIncompatibleDaemon` → PASS
  - Notes: full UI live smoke (`swift run AgtmuxTerm` + provider overlays) は T-XTERM-A5/A6 完了後に final confirmation
  - Scratch handover: `/tmp/agtmux-bootstrap-null-exact-location-handover-20260308.md`

- [x] T-XTERM-A4 (P1) Cross-repo: semantic truth handback for agtmux-term live canaries — DONE (2026-03-08/11)
  - 目的: real-CLI semantic source-of-truth suite を agtmux repo 側で維持しつつ、agtmux-term が薄い daemon-to-sidebar canary を追加できるよう prompt/preflight/oracle 境界を固定する
  - Deliverables (all present in RP):
    - daemon-owned scenario matrix for `provider`, `presence`, `running`, completion state, `waiting_input`, `waiting_approval`, conversation title, and no-bleed
    - provider-specific live prompt guidance for Claude Sonnet 4.6 and Codex 5.4 medium
    - explicit statement that agtmux-term mirrors only boundary assertions, not the full producer semantic matrix
  - Gate: all satisfied; T-XTERM-A3 dependency DONE (2026-03-11)
  - Handover doc: `docs/85_reviews/RP-20260308-agtmux-term-semantic-truth-handover.md`
  - blocked_by: T-XTERM-A3 (DONE)

- [x] T-XTERM-A5 (P0) Cross-repo: managed-exit semantic truth handback — DONE (2026-03-11)
  - 目的: confirmed real Codex pane が no-agent shell (`current_cmd=zsh`) に戻ったあとも stale `presence=managed provider=*` を保持する producer-side semantic drift を止める
  - Root cause: `detect.rs` の `shell_hint` が `process_hint=None + current_cmd=zsh` を見逃し、stale Codex capture tokens で `agent_pane_ids` に残り続けることで step 10c demotion がスキップされていた
  - Fix: `shell_hint` を match に変更して explicit agent hint (`process_hint=Some("claude"/"codex")`) を保護しつつ、`process_hint=None + current_cmd=zsh` を shell として扱う
    ```rust
    let shell_hint = match meta.process_hint.as_deref() {
        Some("claude") | Some("codex") => false,
        Some("shell") => true,
        _ => SHELL_CMDS.contains(&cmd_lower.as_str()),
    };
    ```
  - 変更ファイル 2 件:
    - `crates/agtmux-source-poller/src/detect.rs`: shell_hint fix + 2 new tests
    - `crates/agtmux-runtime/src/poll_loop.rs`: runtime regression test added
  - Gate: `just verify` PASS (213 tests) ✅
  - RP: `docs/85_reviews/RP-T-XTERM-A5-managed-exit-demotion.md`
  - Notes: Phase 3 (cross-repo live smoke) は T-XTERM-A6/A7 完了後に final confirmation
  - blocked_by: T-XTERM-A4 (DONE)

- [ ] T-XTERM-A6 (P0) Cross-repo: app-launched explicit `--tmux-socket` zero-managed-bootstrap handback
  - 目的: `agtmux-term` metadata-enabled XCUITest から spawned された daemon が、exact `--tmux-socket /private/tmp/tmux-501/...` を受け取っているにもかかわらず `ui.bootstrap.v2 total=0`（managed sync-v2 rows が 0）を返す drift を止める
  - Fresh cross-repo repro from `agtmux-term`:
    - metadata-enabled app-driven tmux lane launches:
      - `agtmux --socket-path /Users/virtualmachine/.agt/uit-<token>.sock daemon --tmux-socket /private/tmp/tmux-501/agtmux-managed-<token>`
    - term-side readiness hardening is now landed:
      - `AppViewModel` no longer primes sync-v2 ownership on `inventory present + bootstrap panes=[]`
      - metadata-enabled UI lane now waits for non-empty isolated bootstrap before it launches the live Codex proof
    - latest downstream rerun also proves the app-side harness is no longer the blocker:
      - inventory-only launch no longer dies at `Running Background`
      - delayed metadata enable does spawn the isolated daemon on the custom socket
      - downstream failure summary now shows:
        - `daemonLaunch=spawned:/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux:--socket-path,/Users/virtualmachine/.agt/uit-<token>.sock,daemon,--tmux-socket,/private/tmp/tmux-501/agtmux-managed-<token>`
        - `daemonEnv=... TMUX_BIN=/opt/homebrew/bin/tmux ...`
        - daemon stdout/stderr only:
          - `agtmux daemon starting`
          - `UDS server listening on /Users/virtualmachine/.agt/uit-<token>.sock`
    - same app process probing the custom daemon socket still sees:
      - `ui.bootstrap.v2 total=0 managed=0`
      - `probeTarget=nil`
      - visible app inventory row still present as unmanaged `zsh`
    - stronger downstream evidence now proves the app process itself can speak explicit socket truth:
      - `appDirectSocketProbe=agtmux-e2e-managed-<token>|@0|%0|zsh`
      - i.e. the same app process can run `tmux -S /private/tmp/tmux-501/agtmux-managed-<token> list-panes` and see the isolated pane
      - only the app-child daemon remains stuck at empty bootstrap on that same socket path
    - standalone shell repro with the same local binary and exact daemon args sees the managed pane within 3 seconds
    - stripped-PATH producer repro is now green, and term-side child-daemon launch env is also normalized with:
      - `TMUX_BIN=/opt/homebrew/bin/tmux`
      - normalized `HOME/USER/LOGNAME/XDG_CONFIG_HOME/CODEX_HOME/PATH`
    - despite those controls, the app-launched daemon still never reaches a non-empty bootstrap, so the remaining drift is narrower than a generic PATH/env issue
    - fresh producer-side root cause (2026-03-09):
      - app-like sanitized env causes `tmux list-panes -F` to emit `_`-delimited rows instead of tab-delimited rows
      - `crates/agtmux-tmux-v5/src/pane_info.rs` still parses tab-delimited output only, so inventory fails before managed promotion runs
      - explicit `--tmux-socket` app-child zero-bootstrap is therefore currently an inventory format contract bug, not only a shell-child promotion bug
  - Progress (2026-03-08, Phase 1 landed):
    - added `scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh`
    - wired it into `scripts/tests/e2e/contract/run-all.sh`
    - current expected-red result:
      - tmux server/session/pane comes up after daemon launch
      - tmux side shows `%0 zsh`
      - daemon side still inventories `[]`
    - verification:
      - `bash scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh` → FAIL (expected Phase 1 repro)
      - `bash scripts/tests/e2e/contract/run-all.sh` → `11 passed, 1 failed` with only the new A6 scenario red
  - Phase 1: add a failing producer-side repro that launches the daemon under an app-like/sanitized child-process context with explicit `--tmux-socket` and requires non-empty inventory/bootstrap on the isolated tmux server
  - Phase 2: replace the tmux `list-panes -F` delimiter contract with a printable literal delimiter (`|`) that survives sanitized env, then rerun explicit-socket producer repros
  - Phase 3: rerun the higher-fidelity app-launched / shell-child promotion repros and confirm non-empty managed bootstrap
  - Phase 4: rerun cross-repo UI smoke until the isolated app-child daemon reaches a non-empty bootstrap and `agtmux-term` metadata-enabled plain-zsh Codex lane surfaces the managed row
  - Progress (2026-03-10, Phase 2 confirmed already landed):
    - `crates/agtmux-tmux-v5/src/pane_info.rs` has `LIST_PANES_DELIMITER: char = '|'` and `LIST_PANES_FORMAT` with `|` separators
    - Legacy tab fallback is preserved in `parse_line()`
    - `list_panes_format_uses_printable_pipe_delimiter` and `parse_line_accepts_legacy_tab_delimiter` tests both PASS (15/15 agtmux-tmux-v5 pane_info tests green)
    - Phase 2 fix was part of an earlier commit, not freshly needed
  - Current blocker: `just verify` fails on clippy — see T-VERIFY-FIX
  - Gate:
    - failing producer-side repro added first
    - `just verify` PASS (currently blocked by T-VERIFY-FIX)
    - explicit-`--tmux-socket` app-launched repro no longer returns empty bootstrap
    - cross-repo `agtmux-term` targeted metadata-enabled UI smoke passes
  - Scratch handover: `/tmp/agtmux-app-launched-normalized-env-still-empty-bootstrap-handover-20260308.md`
  - blocked_by: T-XTERM-A5, T-VERIFY-FIX

### Phase 9 — Waiting State Detection Improvements

- [ ] T-codex03a (P3) test-claude-approval.sh Phase 3 に明示的 sleep 追加 — follow-up from RP review
  - Phase 3 で PermissionRequest injector kill 後、`sleep 4` を挿入してから tool_start injector を開始する
  - 現状は暗黙の event expiry (<3s) に依存しており、閾値変更時に false-negative タイムアウトになる可能性
  - blocked_by: なし

- [ ] T-E05a (P3) spinner-title WaitingInput 保護テスト — follow-up from RP review
  - **目的**: `poll_pane_spinner_title_does_not_override_waiting_input` テスト追加
  - **背景**: MT-2 gap — evidence.rs は WaitingInput を capture_lines から生成しないため poller 単体では直接テスト不可
  - **方針**: ActivityState モック or evidence.rs に WaitingInput パターン追加 (将来対応)
  - blocked_by: なし

- [ ] T-codex03b (P3) codex-title.sh online 再検証 — follow-up from RP review
  - `idle → waiting_input` アサーション変更を live Codex セッションで確認する
  - blocked_by: なし

- [x] T-codex03 (P2) waiting_input/waiting_approval 検出修正 — DONE (2026-03-03)
  - **背景**: agtmux-term が `waiting_input`/`waiting_approval` バッジを表示するが、2つのバグで
    これらが `idle`/`unknown` に折りたたまれていた
  - **Fix 1** (Codex): `crates/agtmux-source-codex-jsonl/src/translate.rs` line 79:
    `WaitingInput → activity.idle` → `activity.waiting_input` に修正
  - **Fix 2** (Claude hooks): `crates/agtmux-source-claude-hooks/src/translate.rs`:
    `"Stop" | "SubagentStop" → "activity.waiting_input"` を追加
  - **Fix 3** (e2e contract): `scripts/tests/e2e/contract/test-claude-approval.sh` 新規作成
    (4 フェーズ: tool_start→running, PermissionRequest→waiting_approval, recovery, Stop→waiting_input)
  - **Research**: `docs/research/claude-jsonl-waiting-states.md` — Claude JSONL には
    waiting state シグナルが存在しないため JSONL ソースへの追加は不要
  - **e2e シナリオ更新**: codex-semantic-states/codex-tool-execution/codex-approval-flow/
    codex-session-rotation/codex-title の idle → waiting_input アサーション修正
  - Gate: `just verify` PASS (unit tests); contract e2e 11/11 PASS

### Phase 9 — Codex JSONL Follow-ups (GO_WITH_CONDITIONS conditions)

- [x] T-codex02a (P2) FSM test: WaitingApproval + task_complete → no-op — DONE (2026-03-03)
  - Added `waiting_approval_task_complete_is_noop` test to `fsm.rs`
  - Asserts `transition(WaitingApproval, task_complete) == WaitingApproval`

- [x] T-codex02b (P2) discovery.rs: canonicalize_path /tmp fallback unit test — DONE (2026-03-03)
  - Added `canonicalize_path_tmp_fallback_substitutes_private_tmp` test to `discovery.rs`
  - `#[cfg(target_os = "macos")]` asserts `/private/tmp/...` substitution for non-existent paths

### Phase 7 — Distribution Infrastructure

- [ ] T-D01 (P1) LICENSE + Cargo.toml メタデータ整備
  - `LICENSE` (MIT) をルートに追加
  - workspace `Cargo.toml`: `[workspace.package]` に `license`, `repository`, `edition` を追加
  - `crates/agtmux-runtime/Cargo.toml`: `description`, `keywords`, `categories` を追加
  - blocked_by: なし

- [ ] T-D02 (P1) cargo-dist 設定
  - workspace `Cargo.toml` に `[workspace.metadata.dist]` を追加
  - targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
  - installers: `["homebrew", "shell"]`, tap: `g960059/homebrew-agtmux`
  - `cargo dist init` を実行し生成ファイルを確認
  - blocked_by: T-D01

- [ ] T-D03 (P1) GitHub Actions release workflow
  - `.github/workflows/release.yml`: tag push → verify → cross-compile → GitHub Release → tap 更新 → smoke test
  - `.github/workflows/ci.yml`: PR時 verify のみ
  - Artifact Attestation を有効化
  - blocked_by: T-D02

- [ ] T-D04 (P1) Homebrew formula テンプレート
  - `Formula/agtmux.rb` を作成（cargo-dist が自動生成する場合はその出力を確認）
  - `test do` ブロックに `agtmux --version` を含める
  - blocked_by: T-D02

- [ ] T-D05 (P1) README Install セクション更新
  - 冒頭に brew / curl / cargo の3チャネルを記載
  - アンインストール手順を明記
  - Windows はスコープ外と明記
  - blocked_by: T-D03

## REVIEW
- [ ] (none)

## BLOCKED
- [ ] (none)

## DONE (keep short)
- [x] T-136 (P2) Waiting 表示バグ修正
  - `client.rs` 5箇所で `"Waiting"` → `"WaitingInput" | "WaitingApproval"` 修正。`format_windows` no-color ブランチの `{state}` → `{display_state}` 修正 (同時発見)。2 new tests. 711 → 713 tests. `just verify` PASS.
- [x] T-135a (P3) Codex conversation title 抽出
  - `DaemonState.conversation_titles: HashMap<session_key, String>` を追加。`poll_loop.rs`: Codex events ループで `payload["name"]`/`payload["preview"]` を抽出 → map に挿入。`server.rs`: `build_pane_list` に `conversation_title` フィールド追加。2 new tests. 690 tests total. `just verify` PASS.
- [x] T-134 (P3) `list-windows` リデザイン + `list-sessions` 新規
  - `cmd_list_windows()` に `show_path: bool` 追加。`format_windows()`: @N/@M 完全非表示 (window_name のみ)、%N pane ID 非表示、`[det]`/`[heur]` tag 廃止 → det=無印/heur=`~` prefix 統一、`show_path` サポート、`relative_time` 表示。`cmd_list_sessions()` + `format_sessions()` 新規: session 1行サマリー (N window、M agents、Running/Idle/Waiting、unmanaged count)。`ListSessions(ListSessionsOpts)` を cli.rs に追加 (T-133 で実施済)。12 new tests (format_windows: 1 new + 3 updated; format_sessions: 5 new; format_panes: 8 new). 707 tests total. `just verify` PASS.
- [x] T-133 (P3) `list-panes` 表示リデザイン
  - `cmd_list_panes(json, show_path, color)` に変更。`format_panes()`: session ヘッダー + pane サイドバー (det=無印、heur=`~` yellow、conversation_title or provider 短縮名、relative_time、`--path`/`-p` で current_path 追加)。@N/@M/`%N` ID 完全非表示。`relative_time()`, `resolve_color()`, `provider_short()` ヘルパー追加。`ListPanes(ListPanesOpts)` + `ListSessions(ListSessionsOpts)` を cli.rs/main.rs に追加。
- [x] T-132 (P3) fzf レシピ + README
  - `README.md` 新規作成: 概要・インストール・Quick Start (daemon/hooks/list-windows)・出力フォーマット説明・fzf ワンライナー + `.tmux.conf` スニペット (`bind-key C-w` + `alias aw`)・daemon ライフサイクル・コマンド一覧・`tmux-status` スニペット。`just verify` PASS (688 tests).
- [x] T-131 (P3) `agtmux list-windows` コマンド
  - `cli.rs`: `ListWindows(ListWindowsOpts)` + `--color=always/never/auto`。`client.rs`: `format_windows(panes, use_color)` (unit-testable) + `cmd_list_windows()`。階層表示: session header (N windows — X Running, Y Idle) → window header (@N name — stats) → pane lines (managed: `* provider [det] State path` / unmanaged: `— cmd [unmanaged]`)。Window sort: numeric (@ prefix 除去)。Color auto: `std::io::IsTerminal`。7 new tests. 688 tests total. `just verify` PASS.
- [x] T-130 (P3) `build_pane_list` 不足フィールド追加
  - `session_id`, `window_id`, `current_path` を managed/unmanaged 両方の JSON レスポンスに追加。`TmuxPaneInfo` には既に存在していたが `server.rs` で未露出。2 new tests (managed + unmanaged). 681 tests total. `just verify` PASS.
- [x] T-129 (P2) Supervisor strict wiring
  - `DaemonState.codex_reconnect_failures: u32` を廃止し `codex_supervisor: SupervisorTracker` に置き換え。`should_attempt` チェック: `Ready`→即時試行 / `Restarting{next_restart_ms}` → 時刻比較 / `HoldDown{until_ms}` → 期限確認。成功: `record_success()` / 失敗: `record_failure(now_ms)` → `HoldDown` 時は `warn!`、`Restart` 時は `info!` ログ。4 new tests (initial_ready, failure_advances_restarting, success_resets_ready, budget_exhaustion_holddown). 679 tests total. `just verify` PASS.
- [x] T-128 (P1) [MVP] Process-tree agent identification — `pane_pid` + child-process argv scan
  - `TmuxPaneInfo.pane_pid: Option<u32>` + `LIST_PANES_FORMAT` に `#{pane_pid}` 追加。`scan_all_processes()` (ps -eo) + `inspect_pane_processes_deep()` を capture.rs に実装。`to_pane_snapshot` に `Option<&ProcessMap>` 追加。`pane_tier`: `runtime_unknown` → tier=3 (fail-closed)。poll_loop Step 2.5 で ProcessMap を tick 毎 1 回構築し snapshot に渡す。19 new tests. 675 tests total. `just verify` PASS.
- [x] T-127 (P1) [MVP] Pane attribution false-positive fixes (3 bugs)
  - Bug A: `cwd_candidate_count: usize` in `SessionDiscovery` + `ambiguous_cwd_bootstrap(is_heartbeat=true)` in source.rs。同一 CWD の複数 pane で bootstrap が `last_real_activity[Claude]` を汚染しなくなった
  - Bug B: `CLAUDE_JSONL_RUNTIME_CMDS` positive allowlist in Step 6b filter (poll_loop.rs)。yazi/htop 等 neutral-process pane が JSONL discovery から除外された
  - Bug C: `detect()` shell early return (detect.rs)。`process_hint="shell"` → None。zsh pane の heuristic Claude/Codex 誤帰属を防止
  - 4 new tests: `detect_shell_pane_never_assigned_even_with_claude_output`, `detect_shell_pane_never_assigned_codex`, `discover_sessions_cwd_candidate_count_multi_pane`, `poll_files_emits_ambiguous_bootstrap_when_cwd_has_multiple_panes`. 656 tests total, `just verify` PASS.
- [x] T-126 (P1) [MVP] Claude JSONL all-pane discovery fix (idle session detection after daemon restart)
  - 根本原因: Step 6b が `claude_pane_ids` でゲート → daemon restart 後 projection 空 → discovery なし → heartbeat なし → Codex wins (vicious cycle)
  - Phase 1: `claude_pane_ids` フィルタ廃止 → `snapshot_hint` で process_hint チェック → `Some("shell")|Some("codex")` pane を除外した全候補を `discover_sessions` に渡す (false positive 防止)
  - Phase 2: `SessionFileWatcher.bootstrapped: bool` 追加。初回 poll では `bootstrap_event(is_heartbeat=false)` を emit → `last_real_activity[Claude]` を書き込み、`select_winning_provider` で Codex と比較可能に。2回目以降は従来の `idle_heartbeat(is_heartbeat=true)`
  - Phase 3: Step 6b で `Utc::now()` を使用 (poll_tick の `now` でなく)。Step 6a (Codex network call) より後に呼ぶため T_claude ≥ T_codex → Claude wins provider conflict
  - 3 new/renamed tests: `poll_tick_jsonl_discovery_scans_all_panes`, `poll_files_emits_bootstrap_on_first_poll_when_no_new_lines`, `poll_files_emits_bootstrap_when_only_metadata_lines`. 652 tests total, `just verify` PASS. live 確認: %297 (test-session, idle node) が `claude deterministic idle` に変わることを確認
- [x] T-125 (P1) [MVP] Shell pane false-positive Codex binding fix
  - Evidence: `inspect_pane_processes` に `SHELL_CMDS` リスト (zsh/bash/fish/sh/csh/tcsh/ksh/dash/nu/pwsh) → `Some("shell")` 返却。`pane_tier` に tier 3 追加 (never assign)。`unclaimed` フィルタに `pane_tier < 3` 追加。live 確認: v4 の zsh pane (%286, %305) と test-session %301 (zsh) が unmanaged に変わることを確認。4 new tests。`just verify` PASS (649 tests)。
- [x] T-124 (P1) [MVP] Same-CWD Multi-Pane Codex Binding
  - Evidence: `build_cwd_pane_map` → `build_cwd_pane_groups` (`HashMap<CWD, Vec<PaneCwdInfo>>`). `has_codex_hint: bool` → `process_hint: Option<String>` (3-tier: codex=0/neutral=1/competing=2). `process_thread_list_response` accepts `&[PaneCwdInfo]` + `assigned_in_tick`. H1: generation+birth_ts cache validation. H2: tick-scope dedup. `MAX_CWD_QUERIES_PER_TICK` 8→40. poll_loop.rs updated. 14 new tests (4 groups + 6 assignment + 4 tokio::test). `just verify` PASS (645 tests).
- [x] T-123 (P1) [MVP] Provider Switching — Generic Cross-Provider Arbitration
  - Evidence: `is_heartbeat: bool` added to `SourceEventV2` (serde default=false) + `CodexRawEvent`. Codex poller computes `is_heartbeat=true` when status/pane unchanged and time elapsed ≥2s; all notifications/capture events `is_heartbeat=false`. `DaemonProjection.last_real_activity: HashMap<pane_id, HashMap<Provider, DateTime>>` tracks last non-heartbeat Det event per provider. `select_winning_provider()` picks most-recently-active Det provider; no conflict if ≤1 Det provider. `tick_freshness` clears stale pane entries. Covers Codex→Claude, Claude→Codex, Any→zsh, future Gemini. 10 new tests (8 projection + 2 translate). `just verify` PASS (641 tests).
- [x] T-122 (P1) [MVP] Claude JSONL deterministic source (`agtmux-source-claude-jsonl`)
  - Evidence: New crate `agtmux-source-claude-jsonl` (4 modules: discovery, translate, watcher, source). CWD-based session discovery via `sessions-index.json`. File watcher (EOF seek, partial line, inode rotation). Source rank: `ClaudeHooks(0) > ClaudeJsonl(1) > Poller(2)`. Wired into poll_loop Step 6b + 8c + compaction. Gateway registers 4 sources. 20 new tests. `just verify` PASS (626 tests).
- [x] T-121 (P0) [MVP] Pane-first resolver grouping + pane_generation fallback
  - Evidence: `apply_events()` grouping key changed from `session_key` to `pane_id` (fallback: `session_to_pane` → `session_key`). `resolver_states` keyed by group_key. Per-group multi-session projection. `deterministic_fresh_active` references pane group_key. `pane_generation` fallback from existing pane state. 9 new cross-session tests (3 confirmed bugs → all PASS after fix). ADR-20260226-pane-first-resolver-grouping.md. FR-031a. `just verify` PASS (606 tests).
- [x] T-119 (P1) Codex App Server → pane_id correlation (thread.cwd ↔ tmux pane cwd matching)
  - Evidence: Per-cwd `thread/list` queries (API `cwd` filter param). `PaneCwdInfo` struct + `build_cwd_pane_map()` for disambiguation (Codex process_hint wins). `CodexRawEvent` extended with `pane_generation`/`pane_birth_ts`, passthrough in `translate()`. `poll_threads()` accepts `&[PaneCwdInfo]`, poll_loop builds from `last_panes`+`generation_tracker`+`snapshots`. `FakeTmuxBackend.with_pane_cwd()` for testing. 5 new tests (4 cwd map + 1 translate passthrough). `just verify` PASS (599 tests).
- [x] T-120 (P1) Codex App Server protocol fix + reliability hardening
  - Evidence: B1: `"jsonrpc": "2.0"` on all messages. B2: `"params": {}` on initialized, `"capabilities": {}` on initialize. B3: `used_appserver` based on `is_alive()`. B4: reconnection with exponential backoff (`2^min(failures,6)` ticks), `codex_appserver_had_connection` flag (poll_tick only reconnects dead clients, initial spawn in `run_daemon`). B5: `poll_threads()` outside mutex (take/put pattern). B6: `CodexSourceState.set_appserver_connected()` → health `Healthy`/`Degraded`. C1: deleted `discover_appserver`, `poll_codex_appserver`, `CodexPollerConfig`, `--codex-appserver-addr`. Protocol: `result.data` (not `.threads`), `status.type` (object), `updated_at` (not `lastUpdated`). `just verify` PASS (594 tests).
- [x] T-113a (P1) [MVP] Codex App Server integration: stdio client + capture fallback
  - Evidence: `CodexAppServerClient` (JSON-RPC 2.0 over stdio): spawn `codex app-server`, initialize handshake, `thread/list` polling, `turn/started`/`turn/completed`/`thread/status/changed` notification → `CodexRawEvent`. Capture-based fallback: `parse_codex_capture_events()` extracts NDJSON from tmux capture for `codex exec --json` output. `CodexCaptureTracker` for cross-tick dedup. poll_tick Step 6a: app-server (primary) → capture (fallback). API ref: https://developers.openai.com/codex/app-server/. 12 new tests. `just verify` PASS (597 tests).
- [x] T-118 (P2) [Post-MVP] LatencyWindow → poll tick metrics + path escaping fix
  - Evidence: `LatencyWindow` + `last_latency_eval` wired into DaemonState. poll_tick Step 12: tick timing → SLO evaluation → breach/degraded logging → cached eval. `latency_status` JSON-RPC method (read-only, Codex F4). `shell_quote()` for path escaping in setup_hooks (Codex F5). 5 new tests. `just verify` PASS (570 tests).
- [x] T-115 (P2) [Post-MVP] TrustGuard → UDS admission gate (warn-only)
  - Evidence: `TrustGuard` wired into DaemonState (UID via getuid(), nonce=PID+nanos, 3 sources pre-registered). `source.ingest` schema extended (optional source_id/nonce), warn-only admission (unregistered/nonce mismatch → log, continue). `daemon.info` JSON-RPC method (nonce, version, pid). 5 new tests. `just verify` PASS (585 tests).
- [x] T-117 (P2) [Post-MVP] SourceRegistry → connection lifecycle
  - Evidence: `SourceRegistry` wired into DaemonState. `source.hello` (protocol check + lifecycle), `source.heartbeat`, `list_source_registry` JSON-RPC methods. poll_tick Step 11b: staleness check. 6 new tests. `just verify` PASS (580 tests).
- [x] T-116 (P2) [Post-MVP] CursorWatermarks → gateway cursor pipeline
  - Evidence: `CursorWatermarks` + `InvalidCursorTracker` wired into DaemonState. Step 9a: `advance_fetched()` on gateway pull → `record_valid()` / recovery (RetryFromCommitted/FullResync). Step 11a: `commit()` on gateway commit_cursor. `parse_gw_cursor()` helper. 4 new tests. `just verify` PASS (574 tests).
- [x] T-114 (P1) [MVP] Deterministic session key wiring + CLI title quality
  - Evidence: `PaneRuntimeState.session_key` added, `build_pane_list()` passes `deterministic_session_key` for deterministic panes → `DeterministicBinding` title quality. `summary_changed` includes `deterministic`/`heuristic` counts. 2 new tests. `just verify` PASS (565 tests).
- [x] T-113 (P1) [MVP] Codex appserver poller skeleton
  - Evidence: `codex_poller.rs` with `discover_appserver()` (config + env), `poll_codex_appserver()` (socket check, protocol TBD). `--codex-appserver-addr` CLI option (env: `CODEX_APPSERVER_ADDR`). 4 tests. `just verify` PASS (563 tests).
- [x] T-112 (P1) [MVP] UDS source.ingest + Claude hook adapter + setup-hooks CLI
  - Evidence: `source.ingest` JSON-RPC handler (claude_hooks, codex_appserver dispatch). `scripts/agtmux-claude-hook.sh` (fire-and-forget, jq+socat). `agtmux setup-hooks` CLI (project/user scope, 5 hook types). 9 new tests (4 server + 5 setup_hooks). `just verify` PASS (558 tests).
- [x] T-111 (P1) [MVP] DaemonState expansion + deterministic source pipeline wiring
  - Evidence: codex/claude `compact()` + `compact_offset` added. DaemonState expanded with `codex_source`/`claude_source`. poll_tick steps 8a/8b + compaction. Gateway registers 3 sources. 11 new tests (6 source compact + 5 poll_loop). `just verify` PASS (549 tests).
- [x] T-110 (P1) [MVP] Push event methods: state_changed + summary_changed
  - Evidence: `state_changed` returns version-based changes with pane/session state. `summary_changed` returns managed/unmanaged counts and change flags. Both accept `since_version` param. 4 new tests. `just verify` PASS (536 tests).
- [x] T-109 (P1) [MVP] Title resolver wiring into list_panes API
  - Evidence: `resolve_title()` called in `build_pane_list()` for managed (HeuristicTitle) and unmanaged (Unmanaged fallback) panes. `title` + `title_quality` fields added to JSON response. 1 new test. `just verify` PASS (532 tests).
- [x] T-108 (P1) [MVP] Runtime hardening: API completeness + memory compaction + SIGTERM
  - Evidence: (a) `signature_reason` + `signature_inputs` added to `list_panes` API (FR-024). (b) Poller + gateway buffer compaction wired into poll_loop (compact_offset cursor compatibility). (c) SIGTERM handler added via `tokio::signal::unix`. `just verify` PASS (531 tests = 526 + 5 new).
- [x] T-107 (P1) [MVP] Detection accuracy + activity_state display
  - Evidence: Capture-based 4th detection signal (WEIGHT_POLLER_MATCH=0.78), stale title suppression (title-only + shell + no capture → None), per-pane activity_state + provider in list-panes output. Codex+Claude parallel review adopted (capture tokens tightened: `╭ Claude Code`/`codex>`, shell list expanded: nu/pwsh/tcsh/csh, capture_match wired through payload→poller_match, provider as Option, changed condition updated). `just verify` PASS (525 tests = 514 existing + 11 new).
- [x] T-106 (P1) test strategy + quality gates for runtime crates
  - Evidence: FakeTmuxBackend (mock TmuxCommandRunner) + 12 poll_tick integration tests + 4 build_pane_list unit tests = 16 new runtime tests. E2E smoke script (`just test-e2e-status`). `just verify` PASS (514 tests). `just test-e2e-status` PASS with live tmux.
- [x] T-105 (P1) CLI polish: tmux-status, socket targeting, --poll-interval-ms
  - Evidence: `agtmux tmux-status` outputs `A:4 U:13`. `--tmux-socket`, `AGTMUX_TMUX_SOCKET_PATH/NAME` env supported. `--poll-interval-ms` configurable.
- [x] T-104 (P0) UDS JSON-RPC server + client CLI
  - Evidence: UDS server (connection-per-request, dir 0700, socket 0600, stale cleanup). `agtmux status` connects and prints pane info. 3 methods: list_panes, list_sessions, list_source_health.
- [x] T-103 (P0) poll loop: tmux -> poller -> gateway -> daemon pipeline
  - Evidence: poll_loop.rs wires tmux → poller → gateway → daemon. Unmanaged panes tracked via last_panes + build_pane_list merge. Error recovery (log+skip on capture failure).
- [x] T-102 (P0) runtime skeleton: binary + CLI + daemon + logging
  - Evidence: `agtmux` binary with clap CLI (daemon/status/list-panes/tmux-status). tracing + tracing-subscriber. Signal handling (ctrl_c). `just verify` PASS with 8 crates.
- [x] T-101b (P0) agtmux-tmux-v5: capture + inspection + conversion + generation
  - Evidence: capture_pane, inspect_pane_processes, PaneGenerationTracker (5 tests), to_pane_snapshot (3 tests). cargo test -p agtmux-tmux-v5 PASS.
- [x] T-101a (P0) agtmux-tmux-v5: executor + list_panes parser
  - Evidence: TmuxCommandRunner trait, TmuxExecutor, tab-delimited list_panes parser (10 tests), TmuxPaneInfo, TmuxError (thiserror). cargo test -p agtmux-tmux-v5 PASS.
- [x] T-100a (P0) cursor contract fix: sources always return current position
  - Evidence: 3 sources fixed to always return `Some(current_pos)`. Gateway always overwrites tracker cursor. 2 new no-re-delivery tests added. 471 tests pass.
- [x] T-100 (P0) docs: runtime integration design
  - Evidence: 20_spec.md, 30_architecture.md (C-015/C-016 + MVP topology), 40_design.md (Section 9), 50_plan.md, 60_tasks.md, 90_index.md updated. ADR-20260225-mvp-single-process-runtime.md created. Codex + Opus review adopted.
- [x] T-033 (P2) poller baseline quality spec
  - Evidence: `docs/poller-baseline-spec.md` + `accuracy.rs` 12 tests + fixture 320 windows + `just poller-gate` PASS
- [x] T-041 (P2/P3) cursor contract hardening
  - Evidence: 18 tests pass, two-watermark + safe rewind + invalid cursor streak/resync
- [x] T-043 (P3) latency window SLO gate
  - Evidence: 15 tests pass, rolling p95 + breach counting + degraded alert
- [x] T-047 (P2/P3) UDS trust admission guard
  - Evidence: 15 tests pass, peer uid + source registry + nonce check
- [x] T-048 (P2/P3) source.hello + registry lifecycle
  - Evidence: 18 tests pass, 4-state lifecycle + hello handshake + staleness + socket rotation
- [x] T-049 (P3) snapshot/restore 基盤
  - Evidence: 15 tests pass, snapshot manager + policy + restore dry-run checker
- [x] T-051 (P4) observability alert routing
  - Evidence: 16 tests pass, severity-leveled alert ledger + auto-resolve + policy enforcement
- [x] T-052 (P4) supervisor strict runtime contract
  - Evidence: 18 tests pass, DependencyGate + FailureBudget + HoldDownTimer
- [x] T-053 (P3) binding projection 並行更新制御
  - Evidence: 15 tests pass, single-writer + CAS + conflict retry + rollback prevention
- [x] T-070 (P5) migration/canary/rollback runbook
  - Evidence: `docs/runbooks/migration-canary-rollback.md` + RP-T070
- [x] T-071 (P5) backup/restore runbook
  - Evidence: `docs/runbooks/backup-restore.md` + RP-T071
- [x] T-010 (P0) v5 crate/workspace skeleton
  - Evidence: 6 crates, `just verify` pass
- [x] T-020 (P1) tier resolver + unit/replay
  - Evidence: 35 resolver tests pass, dedup/freshness/rank suppression/re-promotion
- [x] T-011 (P1) poller logic reusable crate
  - Evidence: detect + evidence modules, 24 tests pass
- [x] T-012 (P1) source health FSM
  - Evidence: 31 health transition tests pass, 6-state FSM
- [x] T-013 (P1) title resolver + handshake priority
  - Evidence: 25 title tests pass, 5-tier priority + canonical session
- [x] T-030 (P2) codex appserver source server
  - Evidence: 10 tests pass, translate + source + cursor + health
- [x] T-031 (P2) claude hooks source server
  - Evidence: 11 tests pass, translate + source + cursor clamp fix
- [x] T-032 (P2) poller fallback server
  - Evidence: 40 tests pass, detection + evidence + pagination
- [x] T-040 (P2) gateway aggregation/cursor/health
  - Evidence: 23 tests pass, multi-source merge + cursor + health tracking
- [x] T-044 (P1/P3) pane signature classifier v1
  - Evidence: 27 tests pass, deterministic/heuristic/none + weights + guardrails
- [x] T-045 (P3) signature hysteresis/no-agent demotion
  - Evidence: 25 tests pass, idle/running/demotion windows + flap suppression
- [x] T-042 (P3) pane-first binding state machine
  - Evidence: 34 tests pass, 4-state FSM + generation tracking + tombstone grace + representative selection
- [x] T-050 (P3) daemon v5 projection + client API
  - Evidence: 25 tests pass, list_panes/list_sessions/changes_since + resolver integration
- [x] T-046 (P3) signature fields API exposure
  - Evidence: 9 new tests (34 total daemon), classifier integration + API contract + snapshot tests
- [x] T-060 (P4) supervisor + UI semantics
  - Evidence: 19 tests pass, restart backoff/holddown + startup order + UI labels (agents/unmanaged)
- [x] T-034 (P2) [US-001][US-004] source-specific test scripts を整備
  - Evidence: `scripts/tests/test-source-{codex,claude,poller}.sh` を追加し、`just preflight-online` / `just test-source-*` を実行
  - Notes: testは `/tmp/agtmux-e2e-*` の隔離git workspaceで実行し、完了時に tmux session/workspace/process を cleanup
- [x] T-035 (P2) [US-005] e2e reliability stress (10x + matrix) を実施
  - Evidence: `ITERATIONS=10 WAIT_SECONDS=30 PROMPT_STYLE=compact AGENTS=codex,claude just test-e2e-batch` -> codex 10/10, claude 10/10
- [x] T-009 (P0) [US-005] `just` ベースの local test/quality harness 初期整備
  - Evidence: root `justfile` 追加（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）
- [x] T-001 (P0) [US-005] docs-first baseline を v5 要件で再編
  - Evidence: `docs/00_router.md` 〜 `docs/90_index.md` をテンプレ準拠で再構成
- [x] T-002 (P0) [US-005] v5方針のユーザー確認を反映
  - Evidence: deterministic source固定、JSON-RPC over UDS、`agents` 英語固定、poller 85% baseline 位置づけ
- [x] T-003 (P0) [US-004][US-003] cursor/binding/latency 設計を docs へ反映
  - Evidence: FR-018〜FR-023 を docs に固定
- [x] T-004 (P0) [US-003] pane signature v1 設計を docs へ反映
  - Evidence: FR-024〜FR-031 を docs に固定
- [x] T-005 (P0) [US-005] review 指摘（品質/信頼境界/運用復旧）を docs 契約へ反映
  - Evidence: FR-032〜FR-038 を docs に固定
- [x] T-006 (P0) [US-005] review 指摘（supervisor/ack/registry/FSM並行制御）を docs 契約へ反映
  - Evidence: FR-039〜FR-047 を docs に固定
- [x] T-000 docs skeleton imported from template
  - Evidence: `~/Downloads/docs-first-template/docs` を基に初期構造作成済み

- [x] T-XTERM-A0 (P1) Cross-repo: agtmux-term V2 A0 support (inventory-first UX)
  - 目的: term側のinventory-first renderingを成立させるため、daemon `json` の cached snapshot即時返却と metadata failure non-destructive semantics を固定する
  - Evidence: daemon A0 baseline `09722b7` で cached snapshot即時返却 + metadata failure non-destructive semantics を実装し、agtmux-term A0 baseline `5c5ea10` と cross-repo smoke / compatibility 受け入れを完了
  - Handover: `docs/85_reviews/RP-20260305-agtmux-term-v2-a0-handover.md`
  - blocked_by: なし
- [x] T-XTERM-A1 (P0) Cross-repo: agtmux-term V2 A1 protocol contract (epoch/seq/resync)
  - 目的: daemon -> term の UI 同期を `ui.bootstrap.v2` / `ui.changes.v2` へ固定し、epoch/seq/cursor の曖昧さを排除する
  - Evidence: `crates/agtmux-daemon-v5/src/projection.rs` に strict replay / explicit resync を実装し、`crates/agtmux-runtime/src/server.rs` に `ui.bootstrap.v2` / `ui.changes.v2` を追加。`cargo test -p agtmux-daemon-v5` 151 passed、`cargo test -p agtmux` 160 passed
  - Deliverables:
    - `ui.bootstrap.v2` の bootstrap payload（`epoch`, `snapshot_seq`, `replay_cursor`）を固定
    - `ui.changes.v2` の ordered change feed と `next_cursor` 契約を固定
    - replay miss / epoch mismatch / trimmed cursor で `resync_required` を明示返却
  - Acceptance:
    - silent rewind なしで resync 条件が判定できる
    - same epoch 内で seq continuity が崩れない
    - A2（ack compaction / true stream / observability）を前提にしない
  - Scratch handover: `/tmp/agtmux-v2-daemon-a1-handover-20260305.md`
  - blocked_by: T-XTERM-A0
- [ ] T-XTERM-A2 (P0) Cross-repo: agtmux-term V2 A2 observability + replay ack compaction
  - 目的: replay / overlay / focus / runtime の health を additive に surfacing しつつ、sync-v2 replay を implicit ack で compact できるようにする
  - Evidence: `crates/agtmux-daemon-v5/src/projection.rs` に sync-v2 専用 replay log + ack compaction + replay observability snapshot を追加し、`crates/agtmux-runtime/src/poll_loop.rs` に runtime/focus health state を追加、`crates/agtmux-runtime/src/server.rs` に `ui.health.v1` を追加。`cargo test -p agtmux-daemon-v5` 153 passed、`cargo test -p agtmux` 165 passed
  - Deliverables:
    - sync-v2 専用 replay retention と implicit ack compaction
    - additive `ui.health.v1` (`runtime`, `replay`, `overlay`, `focus`)
    - legacy `state_changed` / `summary_changed` を壊さない change-log 分離
  - Remaining acceptance:
    - agtmux-term A1 consumer が `ui.bootstrap.v2` / `ui.changes.v2` で unchanged pass することを cross-repo で確認（T-XTERM-A3 で compatibility handback 回収後）
    - agtmux-term 側 `ui.health.v1` consumer と接続して UI surfacing を確認
  - Scratch handover: `/tmp/agtmux-v2-a2-cross-repo-handover-20260306.md`
  - blocked_by: T-XTERM-A1, T-XTERM-A3

- [x] T-XTERM-A7 (P0) Cross-repo: exact-row managed demotion and same-session same-provider no-bleed — DONE (2026-03-11)
  - 目的: agent exit 後の shell demotion を exact row に反映し、同一 session 内の sibling Codex pane へ `running` を bleed させない
  - Resolution:
    - Managed-exit demotion: T-XTERM-A5 (`detect.rs` shell_hint fix) により `process_hint=None + current_cmd=zsh` のケースが正しく demote される
    - No-bleed: `scripts/tests/e2e/scenarios/same-session-codex-no-bleed.sh` E2E が存在、`run-all.sh` に含まれる
    - Managed-exit E2E: `scripts/tests/e2e/scenarios/managed-exit.sh` が存在、`run-all.sh` に含まれる（provider-generic）
  - Gate: `just verify` PASS (213 tests via T-XTERM-A5)、online E2E scripts in run-all.sh
  - blocked_by: T-XTERM-A5 (DONE)

- [x] T-XTERM-A8 (P0) Cross-repo: shell demotion when a non-agent child remains under the shell — DONE (2026-03-11)
  - 目的: pane が shell (`current_cmd=zsh`) に戻ったあと、残っている child process が agent ではない場合まで stale managed/provider truth を保持しないようにする
  - Fresh downstream evidence (2026-03-09, after A7 fixes + fresh desktop daemon restart):
    - direct desktop daemon probe no longer shows same-session running bleed:
      - `vm agtmux-term %2=running`
      - `vm agtmux-term %5=waiting_input`
      - `vm agtmux-term %6=waiting_input`
    - but `%6` still reports:
      - `current_cmd=zsh`
      - `presence=managed`
      - `provider=codex`
      - `activity_state=waiting_input`
    - tmux process inspection for the same exact row shows:
      - shell pid `35774`
      - only child process `37202 chezmoi cd`
      - i.e. the pane no longer has a live Codex/Claude child process, but the producer still keeps the managed Codex row
  - Deliverables:
    - exact-row shell demotion does not require the pane to be childless; it only requires that the remaining live child process is not an agent process
    - producer truth demotes `shell + non-agent child` rows back to `presence=unmanaged provider=null activity_state=null|unknown`
    - producer-side live regression/E2E exists for:
      - agent launch
      - forced or natural return to shell
      - replacement by a non-agent child process under the same shell
  - Acceptance:
    - direct `ui.bootstrap.v2` never reports `presence=managed` for a pane whose `current_cmd` is already a shell and whose remaining live child process is not an agent
    - online/live E2E covers the `shell + non-agent child` demotion seam
  - Scratch handover: `/tmp/agtmux-a8-shell-non-agent-child-demotion-20260309.md`
  - blocked_by: T-XTERM-A7

- [x] T-SYNCV3-P2 (P0) Cross-repo: v3 Codex semantic normalization in daemon truth path — DONE (2026-03-09)
  - 目的: v3 truth path で Codex JSONL の旧 collapsed semantics を継承せず、`task_complete` / review mode / tool execution / approval truth を frozen contract に合わせて正規化する
  - Deliverables:
    - `agtmux-source-codex-jsonl` が transition payload に raw Codex semantic trigger (`task_complete`, `entered_review_mode`, `function_call` など) と actual activity timestamp を保持
    - `agtmux-daemon-v5` に Codex v3 normalizer を追加し、`task_complete -> thread.lifecycle=idle + turn.outcome=completed`、`entered_review_mode -> flags.review_mode + pending approval request`、`function_call -> execution=tool_running` を実装
    - review flag 単体では blocking/attention を駆動しないことをテストで固定
  - Evidence:
    - `cargo test -p agtmux-source-codex-jsonl -p agtmux-daemon-v5` PASS
  - Notes:
    - v2 projection / `ActivityState` は未削除
    - live `ui.bootstrap.v3` RPC はこの slice では未接続

- [x] T-SYNCV3-P2-CLAUDE (P0) Cross-repo: v3 Claude field-group authority merge in daemon truth path — DONE (2026-03-09)
  - 目的: Claude hooks/JSONL を frozen sync-v3 contract に合わせて field-group authority split で正規化し、旧 collapsed `ActivityState` semantics を v3 truth path に持ち込まない
  - Deliverables:
    - `agtmux-source-claude-hooks` が `claude_hook` nested payload と actual activity timestamp を保持し、v3 reducer が `PermissionRequest` / `Stop` / `SubagentStop` を識別できる
    - `agtmux-source-claude-jsonl` が `claude_jsonl` nested payload と actual activity timestamp を保持し、JSONL line type を execution/lifecycle hint として扱える
    - `agtmux-daemon-v5` に Claude v3 normalizer を追加し、`PermissionRequest -> pending approval request + waiting_approval`、`Stop/SubagentStop -> idle + completed`、`tool_use/progress -> execution=tool_running` を実装
    - hooks-derived blocking truth が JSONL execution update で上書きされないことと、`Notification(idle_prompt)` が `waiting_user_input` を発明しないことをテストで固定
  - Evidence:
    - `cargo test -p agtmux-source-claude-hooks -p agtmux-source-claude-jsonl -p agtmux-daemon-v5` PASS
  - Notes:
    - Claude v3 truth でも `pending_requests[].request_id` が request identity の唯一の truth
    - live `ui.bootstrap.v3` RPC はこの slice でも未接続

- [x] T-SYNCV3-P3-BOOTSTRAP (P0) Cross-repo: additive `ui.bootstrap.v3` wire from daemon truth — DONE (2026-03-09)
  - 目的: frozen sync-v3 canonical snapshot truth を additive `ui.bootstrap.v3` RPC として公開し、term 側が v2 payload を再解釈せずに bootstrap できるようにする
  - Deliverables:
    - `agtmux-runtime` に live sync-v3 bootstrap builder を追加し、Codex/Claude normalized truth をそのまま `version=3` payload に載せる
    - exact identity fields (`session_name`, `window_id`, `session_key`, `pane_id`, `pane_instance_id`) を strict に維持し、tmux exact identity を解決できない row は v3 output から除外する
    - semantic truth が未ロードの managed pane でも v2 collapsed activity は再利用せず、`managed + unknown/not_loaded` fallback row を daemon 側で生成する
    - `ui.bootstrap.v3` handler を追加し、`ui.changes.v3` は未実装のまま据え置く
  - Evidence:
    - `cargo test -p agtmux` PASS
  - Notes:
    - `ui.bootstrap.v2` / `ui.changes.v2` は未変更
    - live freshness は bootstrap では row-age based summary を使用し、 field-group differential freshness は後続 sliceに deferred

- [x] T-SYNCV3-P3-CHANGES (P0) Cross-repo: additive `ui.changes.v3` wire from daemon truth — DONE (2026-03-09)
  - 目的: `ui.bootstrap.v3` の frozen shape をそのまま継続利用しつつ、sync-v3 canonical row truth から additive な incremental update feed を公開する
  - Deliverables:
    - `agtmux-runtime` に sync-v3 canonical row store + replay cursor + change log を追加し、poll tick ごとに daemon truth から structured field-group update を emit する
    - `ui.changes.v3` handler を追加し、strict identity fields (`session_name`, `window_id`, `session_key`, `pane_id`, `pane_instance_id`) を every upsert/remove payload に保持する
    - upsert は full pane row を返しつつ `field_groups` で changed groups を明示し、remove は exact identity only で返す
    - intermediate v3 semantic truth (`task_complete -> idle+completed`, Claude approval request truth, tool_running execution など) を v2 collapsed activity へ戻さないことをテストで固定する
  - Evidence:
    - `cargo test -p agtmux` PASS
  - Notes:
    - `ui.bootstrap.v2` / `ui.changes.v2` は未変更
    - v3 replay trimming / epoch hardening はこの slice では未実装のため、`ui.changes.v3` は現状 in-memory untrimmed log を返す
    - freshness は引き続き row-age summary ベースで、blocking/execution 別 clock は後続 slice に deferred

- [x] T-SYNCV3-CLEANUP-COMPAT (P1) Daemon cleanup: make `ActivityState` / `activity.*` collapse explicitly sync-v2 compat-only — DONE (2026-03-09)
  - 目的: daemon 側で残っている collapsed `ActivityState` / `activity.*` namespace を old sync-v2 projection / poller boundary に閉じ込め、sync-v3 truth path がそこへ依存しないことをより明示する
  - Deliverables:
    - `agtmux-daemon-v5` projection helper を sync-v2 compat scope が分かる名前/comment に整理する
    - `agtmux-source-poller` の `ActivityState -> event_type` encoder を sync-v2 compat helper として明示する
    - sync-v3 bootstrap test で Codex payload truth が contradictory legacy `event_type` より優先されることを固定する
  - Evidence:
    - `cargo test -p agtmux-daemon-v5 -p agtmux-source-poller -p agtmux sync_v2_compat -- --nocapture` PASS
    - `cargo test -p agtmux sync_v3_runtime::tests::build_bootstrap_ignores_legacy_event_type_when_codex_payload_has_v3_truth -- --nocapture` PASS
  - Notes:
    - sync-v2 transport / replay / CLI-facing activity fields は未削除
    - sync-v3 bootstrap / changes semantics はこの slice で変更しない

- [x] T-SYNCV3-CLEANUP-PAYLOAD-TESTS (P1) Daemon cleanup: make sync-v3 tests payload-first instead of `activity.*`-first — DONE (2026-03-09)
  - 目的: sync-v3 provider/runtime tests で native payload truth が既に存在する箇所について、legacy `activity.*` fixture strings への依存を減らし、compat-only fallback であることをより明示する
  - Deliverables:
    - `codex_v3.rs` / `claude_v3.rs` test helpers を payload-first default に寄せ、legacy compat `event_type` は neutral override として扱う
    - sync-v3 runtime/server helper fixtures も neutral compat `event_type` を default にする
    - poll-loop tests の deterministic Codex JSONL pre-ingest fixtures で empty payload をやめ、actual `payload.codex_jsonl` semantics を持つ event を使う
    - contradictory compat `event_type` でも payload/native semantics が v3 reducer/runtime behavior を駆動することを focused tests で固定する
  - Evidence:
    - `cargo test -p agtmux-daemon-v5 task_complete_ignores_contradictory_compat_event_type_when_payload_truth_exists -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 permission_request_ignores_contradictory_compat_event_type_when_hook_payload_exists -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 jsonl_tool_use_ignores_contradictory_compat_event_type_when_payload_exists -- --nocapture` PASS
    - `cargo test -p agtmux sync_v3_runtime::tests::build_bootstrap_ignores_legacy_event_type_when_codex_payload_has_v3_truth -- --nocapture` PASS
    - `cargo test -p agtmux poll_tick_pulls_from_codex_jsonl_source -- --nocapture` PASS
  - Notes:
    - sync-v2 transport / replay / compat `event_type` field は未削除
    - this slice is test/helper cleanup only; no intended daemon behavior change

- [x] T-SYNCV3-CLEANUP-RUNTIME-V2-WIRE (P1) Runtime cleanup: extract sync-v2 bootstrap/changes builders into a compat-only module — DONE (2026-03-09)
  - 目的: term product path が v3-only になった現状に合わせて、runtime wire layer でも `ui.bootstrap.v2` / `ui.changes.v2` builder logic を compat-only surface として明示する
  - Deliverables:
    - `crates/agtmux-runtime/src/server.rs` から sync-v2 builder logic を compat-only helper/module に抽出する
    - RPC method names / payload shape / ack-compaction behavior は unchanged のまま維持する
    - focused handler tests で sync-v2 ack/compaction が継続しつつ sync-v3 cursor/state を perturb しないことを固定する
  - Evidence:
    - `cargo test -p agtmux ui_bootstrap_v2_handler_compacts_sync_v2_without_touching_sync_v3_cursor -- --nocapture` PASS
    - `cargo test -p agtmux ui_changes_v2_handler_acknowledges_sync_v2_without_touching_sync_v3_cursor -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_handler_does_not_compact_sync_v2_log -- --nocapture` PASS
  - Notes:
    - sync-v2 transport / replay payloads are still present for compatibility
    - no `ui.bootstrap.v3` / `ui.changes.v3` semantic changes in this slice

- [x] T-SYNCV3-CLEANUP-SOURCE-V2-EVENT-TYPES (P1) Source cleanup: extract sync-v2 compat `event_type` mapping behind shared helpers — DONE (2026-03-09)
  - 目的: source modules を v3-payload-first な読み味に保ちつつ、legacy `activity.*` / `tool_complete` / `user_input` / `lifecycle.*` string generation を explicit な sync-v2 compatibility layer に閉じ込める
  - Deliverables:
    - `agtmux-core-v5` に shared `sync_v2_compat` helper module を追加し、poller / Claude hooks / Claude JSONL で使う legacy `event_type` mapping を一箇所に集約する
    - `agtmux-source-poller` と touched translators (`agtmux-source-claude-hooks`, `agtmux-source-claude-jsonl`, `agtmux-source-codex-jsonl`) が inline string tables を持たずに compat helper を呼ぶ形へ整理する
    - Claude/Codex idle bootstrap/heartbeat path でも raw compat string を helper 経由に寄せ、emitted strings と source behavior は unchanged のまま維持する
  - Evidence:
    - `cargo test -p agtmux-core-v5 sync_v2_compat -- --nocapture` PASS
    - `cargo test -p agtmux-source-poller sync_v2_compat_activity_state_mapping_to_event_type -- --nocapture` PASS
    - `cargo test -p agtmux-source-claude-hooks event_type_normalization -- --nocapture` PASS
    - `cargo test -p agtmux-source-claude-jsonl translate::tests -- --nocapture` PASS
    - `cargo test -p agtmux-source-codex-jsonl translate::tests -- --nocapture` PASS
  - Notes:
    - sync-v2 transport / replay / legacy `event_type` fields are still present for compatibility
    - this slice does not change v3 reducer truth or source-native payload contents

- [x] T-SYNCV3-CLEANUP-PROJECTION-V2-PARSER (P1) Daemon cleanup: move sync-v2 compat `event_type` parsing into shared core helper — DONE (2026-03-09)
  - 目的: projection がまだローカル所有している legacy `event_type -> ActivityState` parser を `agtmux-core-v5::sync_v2_compat` に寄せ、source-side compat helper と同じ shared boundary に揃える
  - Deliverables:
    - `agtmux-core-v5::sync_v2_compat` に legacy parse helper を追加し、`activity.*` / `lifecycle.*` / `thread.*` / `turn.*` aliases をそこで一元管理する
    - `agtmux-daemon-v5::projection` は shared parser を consume するだけにし、ローカル duplicate parser を削除する
    - parsing behavior / projection behavior / sync-v3 semantics は unchanged のまま維持する
  - Evidence:
    - `cargo test -p agtmux-core-v5 sync_v2_compat -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 sync_v2_compat_activity_state_parsing -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 heartbeat_on_new_pane_sets_initial_activity -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 real_stop_event_correctly_sets_idle -- --nocapture` PASS
  - Notes:
    - no sync-v3 reducer/runtime semantics changed in this slice
    - broader `ActivityState` deletion remains deferred

- [x] T-SYNCV3-CODEX-WAITING-INPUT-PROOF (P1) Producer proof: Codex `task_complete` intentionally diverges between sync-v2 and sync-v3 surfaces — DONE (2026-03-09)
  - 目的: live Codex lane で `agtmux json` が `waiting_input` を示しつつ `ui.bootstrap.v3` が `completed_idle` を返す件について、consumer reinterpretation ではなく producer-owned contract divergence であることを source/runtime path から明示する
  - Deliverables:
    - same exact Codex `task_complete` source event を projection と sync-v3 runtime の両方へ流し、sync-v2/list/json path は `activity_state=WaitingInput` を保持しつつ `ui.bootstrap.v3` は `thread.lifecycle=idle + turn.outcome=completed` を返す focused proof test を追加する
    - docs に `task_complete` は `waiting_user_input` を発明しないという frozen v3 contract を term blocker 向けに明記する
  - Evidence:
    - `cargo test -p agtmux codex_task_complete_intentionally_diverges_between_sync_v2_and_v3_surfaces -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_emits_strict_identity_and_normalized_codex_truth -- --nocapture` PASS
    - `cargo test -p agtmux-daemon-v5 task_complete_normalizes_to_idle_completion_without_blocking -- --nocapture` PASS
    - `cargo test -p agtmux-source-codex-jsonl poll_files_emits_waiting_input_on_task_complete -- --nocapture` PASS
  - Notes:
    - by design, `task_complete` does not imply `waiting_user_input` in sync-v3 without an explicit unresolved input request entity
    - term must not infer `waiting_user_input` from Codex `completed_idle` rows alone

- [x] T-XTERM-A6b (P0) Producer fix: app-child exact-socket Codex promotion survives stripped PATH metadata tools — DONE (2026-03-09)
  - 目的: tmux inventory は exact socket で見えているのに `ui.bootstrap.v3` では `shell:%pane` unmanaged row しか出ない blocker を producer-side metadata pipeline で解消する
  - Deliverables:
    - shared producer-side system binary resolver を追加し、app-child / XCUITest 由来の stripped PATH でも `ps` と `lsof` を標準 system path fallback で解決する
    - `scan_all_processes()` と JSONL discovery の `lsof` call sites を helper 経由に寄せ、tmux inventory だけ成功して managed promotion が fail-closed になる drift をなくす
    - docs / tests で「sync-v3 bootstrap が unmanaged なのは row composition の collapse ではなく producer managed-truth 未形成だった」ことを固定する
  - Evidence:
    - `cargo fmt --all` PASS
    - `cargo test -p agtmux-core-v5 system_bin -- --nocapture` PASS
    - `cargo test -p agtmux-tmux-v5 snapshot_deep_inspection_shell_descendant_codex -- --nocapture` PASS
    - `cargo test -p agtmux-source-codex-jsonl get_cwd_via_lsof_invalid_pid_returns_none -- --nocapture` PASS
    - `cargo test -p agtmux-source-claude-jsonl discover_jsonl_via_lsof_nonexistent_pid_returns_none -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS
  - Notes:
    - no sync-v3 semantics changed in this slice
    - this restores producer managed-truth formation so bootstrap can surface the row that already belongs on the exact socket

- [x] T-XTERM-A6c (P0) Producer proof: exact-socket shell inventory is not managed truth before Codex launch — DONE (2026-03-09)
  - 目的: term 側 targeted UI lane が `codex exec` 送信前の bootstrap-ready gate で `presence=managed provider!=nil` を待っている件について、same-pane inventory と `ui.bootstrap.v3` を同時点比較し、producer semantics が plain `zsh` inventory を unmanaged のまま返すことを固定する
  - Deliverables:
    - runtime test で cached inventory row (`current_cmd=zsh`) と `ui.bootstrap.v3` row が同一 pane identity を共有しつつ、provider truth 未到着の間は `session_key=shell:%pane` / `presence=unmanaged` を返すことを証明する
    - 同じ pane に Codex source event が入った後だけ `presence=managed provider=codex` へ遷移することを同じ test 内で固定する
    - docs に「exact socket pane visibility does not imply managed bootstrap before provider truth arrives」を追記する
  - Evidence:
    - `cargo fmt --all` PASS
    - `cargo test -p agtmux plain_shell_inventory_remains_unmanaged_in_bootstrap_until_provider_truth_arrives -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_emits_strict_identity_and_normalized_codex_truth -- --nocapture` PASS
  - Notes:
    - appDirectSocketProbe proving `tmux -S <socket> list-panes` can see `%0 zsh` is inventory truth only
    - managed/provider truth begins after Codex source semantics arrive, not before `codex exec` is sent

- [x] T-XTERM-A6d (P0) Repo-owned proof: exact-socket Codex mid-flight sync-v3 truth exists before completion — DONE (2026-03-09)
  - 目的: flaky XCUITest に依存せず、app-like explicit `--tmux-socket` daemon env で長めの real Codex task 中の same-pane truth を repo-owned scenario で固定し、producer miss と post-completion timing/demotion を切り分ける
  - Deliverables:
    - codex-specific exact-socket e2e scenario を追加し、5-10 秒の mid-flight 時点で同一 pane について:
      - tmux exact-socket row (`session|window|pane|pid|current_command`)
      - daemon `list_panes_snapshot`
      - daemon `ui.bootstrap.v3`
      を同時採取する
    - 同シナリオで completion 後 snapshot も残し、managed-completion or shell-demotion のどちらになったかを明示する
    - harness に generic daemon JSON-RPC helper を追加して v3 wire proof を shell scenario から直接取得できるようにする
  - Evidence:
    - `bash -n scripts/tests/e2e/harness/common.sh` PASS
    - `bash -n scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh` PASS
    - `bash -n scripts/tests/e2e/online/run-all.sh` PASS
    - `PROVIDER=codex bash scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh` PASS
  - Notes:
    - observed proof on this machine:
      - mid-flight @8s: tmux exact-socket row and both daemon surfaces agreed on the same pane identity, with `list_panes_snapshot` + `ui.bootstrap.v3` both `presence=managed provider=codex`
      - immediate post-completion snapshot still showed managed Codex completion truth (`activity_state=WaitingInput` in sync-v2 compat, `thread.lifecycle=idle + turn.outcome=completed` in sync-v3)
    - this means the remaining term-side red was not reproduced as a producer miss in repo-owned exact-socket proof

- [x] T-XTERM-A6e (P0) Producer fix: Codex node-runtime discovery no longer requires a direct process hint — DONE (2026-03-09)
  - 目的: f559187 exact-socket proof と term T-149 contradiction を reconcile し、exact socket で `pane_current_command=node` を観測しても app-child bootstrap が `shell:%pane` unmanaged のまま固着する drift を producer-side runtime で解消する
  - Deliverables:
    - Codex JSONL discovery を `process_hint=codex` 専用ゲートから広げ、neutral runtime `node` + tmux current_path でも candidate に含める
    - Codex JSONL discovery を coarse `metadata_failure_reason` gate から外し、deep process inspection が degraded な tick でも tmux/CODEX_HOME 側の deterministic truth を引けるようにする
    - Codex session root resolution を `CODEX_HOME` 優先にし、app-child env と shell proof env の差で `~/.codex/sessions` を取り逃がさないようにする
  - Evidence:
    - `cargo test -p agtmux poll_tick_discovers_codex_jsonl_from_node_runtime_without_process_hint -- --nocapture` PASS
    - `cargo test -p agtmux codex_jsonl_candidates_include_neutral_node_runtime -- --nocapture` PASS
    - `cargo test -p agtmux-source-codex-jsonl codex_home_dir_ -- --nocapture` PASS
  - Notes:
    - explicit proof path and term app-child path still differed materially in one place: shell proof already had direct Codex process truth, while app-child could remain at `current_cmd=node` and needed JSONL/CWD truth to promote
    - this slice keeps sync-v3 semantics unchanged; it only hardens producer managed-truth formation for Codex node runtimes

- [x] T-XTERM-A6f (P0) Producer fix: preserve linked-session v3 row identity and document shell→managed promotion identity churn — DONE (2026-03-09)
  - 目的: linked-session/app-child path で same live pane が複数 exact tmux locations に現れる場合でも、sync-v3 bootstrap/changes が `pane_id` 単独で collapse せず full exact location (`session_name`, `window_id`, `pane_id`) ごとに row truth を返すようにし、term 側 investigation 向けに shell bootstrap → managed promotion の identity pattern も固定する
  - Deliverables:
    - `SyncV3LiveState` row store/reconcile を exact location key に切り替え、同一 `pane_id` が linked session に現れる場合は v3 bootstrap/changes が各 location row を保持できるようにする
    - focused runtime/server tests で:
      - linked-session managed pane が `ui.bootstrap.v3` では 2 rows に fan-out する一方、sync-v2 compat `list_panes_snapshot` は従来どおり 1 row compaction のままであること
      - same visible shell row が later `ui.changes.v3` upsert で managed Codex row に昇格する際、`pane_instance_id` は stable だが `session_key` は `shell:%pane` → `codex:%pane` に変わること
    - Step 6a candidate helper で `process_hint=runtime_unknown` かつ `current_cmd=node` も Codex JSONL discovery candidate に含める
    - daemon-owned contract docs に linked-session exact identity / shell→managed identity churn を追記する
  - Evidence:
    - `cargo test -p agtmux build_bootstrap_preserves_linked_session_locations_for_same_pane_id -- --nocapture` PASS
    - `cargo test -p agtmux reconcile_removes_only_missing_linked_session_location -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_preserves_linked_session_rows_even_when_v2_cache_compacts -- --nocapture` PASS
    - `cargo test -p agtmux ui_changes_v3_promotes_same_visible_row_with_stable_pane_instance_id -- --nocapture` PASS
    - `cargo test -p agtmux codex_jsonl_candidates_include_neutral_node_runtime -- --nocapture` PASS
  - Notes:
    - sync-v2 compat surfaces intentionally still compact managed linked-session rows by `pane_id`; this slice changes only sync-v3 exact-row truth
    - shell bootstrap → managed promotion at the same visible location still changes `session_key` by design; term consumer logic must tolerate that upsert pattern instead of treating it as an impossible conflict

- [x] T-XTERM-A6g (P0) Producer fix: replace exact-identity churn with remove+upsert in `ui.changes.v3` — DONE (2026-03-09)
  - 目的: unmanaged shell row が same visible location で managed Codex/Claude row に昇格する際、daemon が conflicting exact identity を single upsert として流していた問題を修正し、strict consumer が old exact row を明示的に落としてから new exact row を受け取れるようにする
  - Deliverables:
    - sync-v3 reconcile で `session_key` / `pane_instance_id` を含む exact identity field が同じ visible location 上で変化した場合、`ui.changes.v3` は `remove(old exact identity)` + `upsert(new exact identity)` を emit する
    - focused runtime/server tests で shell bootstrap → managed promotionが:
      - bootstrap では unmanaged shell row
      - changes では old shell remove + new managed upsert
      - `pane_instance_id` は stable
      - `freshness.down` は provider state ではない
      ことを固定する
    - daemon-owned contract docs に exact-identity change の replace semantics を追記する
  - Evidence:
    - `cargo test -p agtmux build_changes_replaces_row_when_exact_identity_changes_at_same_location -- --nocapture` PASS
    - `cargo test -p agtmux build_changes_replaces_row_when_claude_promotion_changes_exact_identity -- --nocapture` PASS
    - `cargo test -p agtmux ui_changes_v3_replaces_shell_row_when_exact_identity_changes_on_promotion -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS
    - `cargo test -p agtmux ui_changes_v3_emits_upsert_with_strict_identity_from_sync_v3_truth -- --nocapture` PASS
  - Notes:
    - linked-session row fan-out from T-XTERM-A6f remains in place; this slice only changes same-location exact-identity churn behavior in the v3 changes lane
    - provider truth is still driven by `presence` / `provider` / thread fields, not by freshness badges alone

- [x] T-XTERM-A6h (P0) Producer fix: Codex JSONL HOME fallback resolves to `~/.codex` so reducer truth can replace managed fallback inactive — DONE (2026-03-09)
  - 目的: `CODEX_HOME` unset の live harness でも Codex transcripts を `~/.codex/sessions` から discover し、sync-v3 exact row が `managed/not_loaded` fallback に留まらず reducer-backed running truth へ昇格できるようにする
  - Fresh producer proof (2026-03-09):
    - term-like 2-pane interactive repro では exact Codex row が `provider=codex presence=managed` まで surfacing する一方、`list_source_health` の `codex_jsonl` は run 中ずっと `down` のままだった
    - 同じ run で matching transcript file 自体には `task_started`, `user_message`, `function_call`, `function_call_output`, `task_complete` が存在した
    - root cause は `agtmux-source-codex-jsonl::discovery::codex_home_dir_from_env()` が `HOME` fallback を `~/sessions` 扱いしており、実際の `~/.codex/sessions` を scan していなかった点
  - Deliverables:
    - `codex_home_dir_from_env()` fallback を `HOME/.codex` に修正
    - source unit test で HOME-only env が `~/.codex` を返すことを固定
    - runtime poll tick test で `CODEX_HOME` unset + `HOME/.codex/sessions` transcript から Codex pane が sync-v3 `thread.lifecycle=active` に到達することを固定
  - Evidence:
    - `cargo test -p agtmux-source-codex-jsonl codex_home_dir_falls_back_to_home_env -- --nocapture` PASS
    - `cargo test -p agtmux poll_tick_discovers_codex_jsonl_via_home_dot_codex_fallback -- --nocapture` PASS
  - Notes:
    - this slice fixes discovery/binding only; Codex FSM semantics stay unchanged because the matched live transcript already contained `task_started` / `function_call` and was simply never discovered

- [x] T-SYNCV3-FRESHNESS-FALLBACK (P1) Daemon fix: recent managed fallback rows no longer force `freshness.down` by construction — DONE (2026-03-10)
  - 目的: sync-v3 provider-attributed rowsが reducer truth未ロードの fallback (`thread.lifecycle = not_loaded`) にいる間でも、recent `updated_at` を持つ row を `down/down/down` で誤表示しないようにする
  - Deliverables:
    - `build_managed_fallback_snapshot()` の freshness を hard-coded `down/down/down` から `managed.updated_at` ベースの row-age summary に変更
    - focused runtime tests で:
      - fallback-managed row が collapsed v2 `activity_state` を semantic truth として再利用しないこと
      - fallback-managed row が recent `updated_at` では `fresh/stale` を返し、十分に古い場合だけ `down` へ落ちること
      - `ui.bootstrap.v3` surface でも managed fallback row が `thread.not_loaded + freshness=fresh` で返ること
  - Evidence:
    - `cargo test -p agtmux sync_v3_runtime::tests::managed_fallback_does_not_reuse_collapsed_v2_activity_state -- --nocapture` PASS
    - `cargo test -p agtmux sync_v3_runtime::tests::managed_fallback_freshness_tracks_projection_updated_at -- --nocapture` PASS
    - `cargo test -p agtmux ui_bootstrap_v3_managed_fallback_ages_freshness_from_projection_updated_at -- --nocapture` PASS
  - Notes:
    - Cause 1 fixed here: managed fallback rows are no longer born `freshness.down` solely because reducer truth is missing
    - Cause 2 remains open by design in this slice: reducer-backed idle/waiting rows still age to `down` after `>15s` via the existing row-age summary policy
    - downstream live regression pin remains the existing term-side managed-provider live proof after this daemon fix

---

### sync-v2 removal planning (2026-03-10 Codex analysis)

**Classification summary** (from Codex read-only survey):
- `ActivityState` 型自体は sync-v3 でも使用 → 削除対象外
- compat-only (削除候補): 5 source adapter ファイルの `sync_v2_compat::activity_event_type()` 呼び出し
- product-path (後回し): `ui.bootstrap.v2` / `ui.changes.v2` RPC endpoints — T-XTERM-A3〜A8 完了後まで削除不可
- never-remove: `ActivityState` enum、hysteresis、CLI コマンド、projection core

**削除順序**:
1. Phase 1 (compat-only transport adapters) — T-SV2-P1 — safe, no protocol change
2. Phase 2 (v2 RPC endpoints) — T-SV2-P2 — blocked_by T-XTERM-A3〜A8 + agtmux-term v3-only migration
3. `agtmux-core-v5::sync_v2_compat` module itself — T-SV2-P3 — blocked_by T-SV2-P2

- [x] T-SV2-P1 (P2) sync-v2 compat: remove event_type string round-trip from source adapters — DONE (2026-03-11)
  - `SourceEventV2.event_type: String` → `activity_state: ActivityState` に置き換え（17 ファイル）
  - source adapters が `ActivityState` を直接設定; projection が `event.activity_state` を直接参照
  - Gate: `just verify` PASS (967 tests) ✅
  - RP: `docs/85_reviews/RP-T-SV2-P1-event-type-round-trip-removal.md`

- [ ] T-SV2-P2 (P2) sync-v2 compat: remove `ui.bootstrap.v2` / `ui.changes.v2` RPC endpoints
  - 目的: `agtmux-term` が sync-v3 only に移行完了した後、daemon から v2 wire endpoints を削除し、`agtmux-runtime::sync_v2_compat` module ごと除去する
  - 対象:
    - `crates/agtmux-runtime/src/server.rs` — `ui.bootstrap.v2` / `ui.changes.v2` handlers
    - `crates/agtmux-runtime/src/sync_v2_compat.rs` — `build_ui_bootstrap_v2()` / `build_ui_changes_v2()` / `build_sync_v2_pane_list()`
  - Gate: agtmux-term の全 live tests が `ui.bootstrap.v3` / `ui.changes.v3` のみで通過すること
  - blocked_by: T-XTERM-A3, T-XTERM-A4, T-XTERM-A5, T-XTERM-A7, T-XTERM-A8 (agtmux-term full v3 migration)

- [ ] T-SV2-P3 (P2) sync-v2 compat: delete `agtmux-core-v5::sync_v2_compat` module
  - 目的: T-SV2-P1 + T-SV2-P2 完了後、core module 自体を除去する
  - 対象: `crates/agtmux-core-v5/src/sync_v2_compat.rs` + `lib.rs` からの re-export
  - blocked_by: T-SV2-P1, T-SV2-P2
