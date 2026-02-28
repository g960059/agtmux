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

## DOING
- [ ] (none)

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
