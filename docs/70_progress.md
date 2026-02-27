# Progress Ledger (append-only)

## Rules
- Append only. 既存履歴は書き換えない。
- 記録対象: 仕様変更、判断、ユーザー要望、学び、gate証跡。

---

## 2026-02-26 (cont.)
### T-123: Provider Switching — Generic Cross-Provider Arbitration

### Completed
- `is_heartbeat: bool` field added to `SourceEventV2` (with `#[serde(default)]`) and `CodexRawEvent`
- Codex poller: `is_heartbeat=true` when status+pane unchanged and elapsed ≥ `HEARTBEAT_INTERVAL_SECS` (2s); all notifications and capture events use `is_heartbeat=false`
- `DaemonProjection.last_real_activity: HashMap<pane_id, HashMap<Provider, DateTime<Utc>>>`: updated only for non-heartbeat Det events in `apply_events`
- `select_winning_provider()`: when ≤1 Det provider in batch → no-op (return that provider); when multiple → winner = most-recent real activity in `last_real_activity`; fallback = current pane provider or latest event
- `tick_freshness`: removes stale pane entries from `last_real_activity`
- 10 new tests: 8 in projection.rs + 2 in translate.rs (codex-appserver)
- 641 tests total (up from 631), all PASS

### Key decisions
- **pane_title 使用禁止** (ユーザー指示): binding 判定・provider 切替検出・generation bump のすべてに使用禁止。ADR-20260225 および docs/40_design.md に記録済み。
- **正しい検出手法**: `is_heartbeat` フラグ + `last_real_activity` per-pane per-provider tracking。Codex heartbeat は freshness 維持のみで provider winner 選択には使わない。
- **Resolver 変更なし**: tier 選択ロジックは resolver に残し、cross-provider 競合解決は projection 層で行う設計。
- **汎用設計**: Gemini/Copilot などの将来の provider も `Provider` enum への追加だけで対応可能。

---

## 2026-02-26 (cont.)
### Current objective
- Bugfix: Detection accuracy — WaitingApproval false positive + provider misidentification

### Completed
- **Detection accuracy bugfix**: ライブテストで 2 つの検出精度バグを発見・修正
  - **Bug 1 — WaitingApproval 偽陽性**: Claude Code のステータスバー `"⏵⏵ bypass permissions on"` が `"permission"` パターンにマッチし、全 idle Claude pane が WaitingApproval と誤判定
    - **Fix**: `evidence.rs` の WaitingApproval パターンを具体的な UI プロンプトに限定
      - Claude: `["Allow?", "Do you want to allow"]` (旧: `["Allow?", "approve", "permission"]`)
      - Codex: `["Apply patch?"]` (旧: `["approve", "confirm"]`)
  - **Bug 2 — Provider 誤識別**: v4 session の Codex pane が stale な `pane_title="✳ Claude Code"` により Claude と誤検出
    - **Fix**: `detect.rs` の title-only 抑制を無条件化 — title_match のみでは `current_cmd` に関係なく検出しない
    - 削除: `KNOWN_SHELLS` 定数、`cmd_basename()`, `is_known_shell()` 関数 (不要になった dead code)
  - **Fixture 更新**: `dataset.json` の 43 capture lines を現実的な UI パターンに置き換え (`random.seed(42)` で決定的)
  - **Test 更新**: 6 テスト削除 (shell-specific suppression)、2 テスト追加 (title-only unconditional suppression)、4 テスト修正
  - Files: `evidence.rs`, `detect.rs`, `accuracy.rs`, `fixtures/poller-baseline/dataset.json`
  - Live test verified: Claude panes → Idle, v4 Codex → title-only suppressed, 597 tests pass
  - Docs 更新: `40_design.md` (title-only 抑制), `20_spec.md` (FR-027), `poller-baseline-spec.md` (signal weights), ADR (guardrails)

### Key decisions
- `pane_title` は単独シグナルとして信頼できない — stale title がプロセス変更後も残存するため、title_match のみの検出は無条件で抑制
- WaitingApproval パターンは具体的な UI プロンプト文字列に限定 — 汎用的な単語 (`"permission"`, `"approve"`) は status bar 等の無関係なコンテキストにマッチする

### Learnings
- Claude Code のステータスバー (`bypass permissions on`) は activity 検出ではなく UI 設定表示 — activity signal pattern は UI プロンプトの exact phrase に限定すべき
- tmux の `pane_title` はプロセス変更時に更新されない場合がある — v4 session で Codex に切り替わっても旧 Claude の title が残存

---

## 2026-02-26 (cont.)
### Current objective
- Bugfix: Codex pane `activity_state: Unknown` in live CLI output

### Completed
- **Codex activity_state Unknown bugfix**: Real Codex App Server (v0.104.0) does NOT include `status` field in `thread/list` responses — all threads defaulted to "unknown" status → `ActivityState::Unknown`.
  - **Root cause**: `process_thread_list_response()` in `codex_poller.rs` used `.unwrap_or("unknown")` for missing status, but the real API omits `status` entirely from thread/list results (only guaranteed in `thread/status/changed` notifications and `thread/read`).
  - **Fix 1 (root cause)**: Changed default from `"unknown"` to `"idle"` — a listed thread is at least available/loaded.
  - **Fix 2 (notLoaded filter)**: Skip `notLoaded` threads in `process_thread_list_response()` — these are historical threads on disk, not in memory.
  - **Fix 3 (defensive)**: Added `"thread.not_loaded"` → `ActivityState::Idle` in `parse_activity_state()`.
  - **Fix 4 (clippy)**: Collapsed nested `if let Some(events) ... { if !events.is_empty() {` into single condition in poll_loop.rs.
  - **External enhancements** (applied during session): `session_to_pane` HashMap in projection.rs for pane_id fallback, `ThreadPaneBinding`/`LastThreadState` in codex_poller.rs, per-cwd query limits (`MAX_CWD_QUERIES_PER_TICK=8`, `THREAD_LIST_REQUEST_TIMEOUT=500ms`).
  - Files: `codex_poller.rs`, `projection.rs`, `poll_loop.rs`
  - Live test verified: all Codex panes show `activity_state: Idle`, Claude panes show appropriate states.
  - `just verify` PASS — 601 tests, 0 failures, fmt + clippy clean.

### Key decisions
- Default to `"idle"` (not `"unknown"`) when Codex App Server omits `status` from `thread/list` — a listed thread is at minimum available.
- `notLoaded` threads are filtered at the poller level (not projection) since they represent unavailable historical threads.

### Learnings
- Codex App Server API documentation vs reality gap: `thread/list` response schema shows `status: { type: "idle" }` but real v0.104.0 responses omit the field entirely.
- Debug logging (`raw_status=None`) was the key technique to discover the root cause — initial hypothesis about `notLoaded` threads was a contributing factor but not the primary issue.

---

## 2026-02-26 (cont.)
### Current objective
- Codex App Server 公式 API ドキュメントの永続化

### Completed
- **Codex API reference 永続化**: コンパクション時に公式 API 情報が失われ、独自実装に逸脱する問題を解決
  - `docs/codex-appserver-api-reference.md` 新規作成: 公式 API の全メソッド・通知・スキーマ・実装方針・既知問題を記録
  - `docs/40_design.md`: Codex App Server Integration セクション追加 (architecture diagram, primary/fallback path, poll_tick Step 6a)
  - `docs/00_router.md`: External API References セクション追加 (Codex 実装時の必読指示)
  - `docs/90_index.md`: API reference への導線追加
  - `CLAUDE.md`: Codex API reference 必読指示追加
  - 調査で判明した現実装の問題点: `jsonrpc: "2.0"` フィールド欠落、`used_appserver` フラグバグ、再接続なし

### Key decisions
- 公式 API 仕様は `docs/codex-appserver-api-reference.md` に永続化し、コンパクション耐性を確保する
- 独自プロトコルの新規実装は禁止。capture fallback は既存のみ維持。

---

## 2026-02-26 (cont.)
### Current objective
- T-119: Codex App Server → pane_id correlation

### Completed
- **T-119**: pane_id correlation via per-cwd `thread/list` queries
  - `PaneCwdInfo` struct: pane_id, cwd, generation, birth_ts, has_codex_hint
  - `build_cwd_pane_map()`: deduplicates by cwd, Codex process_hint wins disambiguation
  - `poll_threads(&[PaneCwdInfo])`: issues per-cwd `thread/list` requests with API `cwd` filter param
  - `CodexRawEvent` extended with `pane_generation`/`pane_birth_ts` fields, passthrough in `translate()`
  - poll_loop builds PaneCwdInfo from `last_panes` + `generation_tracker` + `snapshots`
  - `FakeTmuxBackend.with_pane_cwd()` for testing with specific pane cwds
  - 5 new tests: 4 cwd map disambiguation + 1 translate passthrough
  - `just verify` PASS (599 tests)

---

## 2026-02-26 (cont.)
### Current objective
- T-120: Codex App Server protocol fix + reliability hardening

### Completed
- **T-120**: Protocol compliance + reliability + health propagation (B1-B6, C1)
  - **B1**: `"jsonrpc": "2.0"` on all outgoing messages (initialize, initialized, thread/list)
  - **B2**: `"params": {}` on initialized notification, `"capabilities": {}` on initialize
  - **B3**: `used_appserver` flag based on `is_alive()` not event count → no spurious capture fallback
  - **B4**: Reconnection with exponential backoff (`2^min(failures,6)` ticks). `codex_appserver_had_connection` flag ensures poll_tick only reconnects previously-alive clients; initial spawn happens in `run_daemon`.
  - **B5**: `poll_threads()` called outside mutex (take/put pattern) → DaemonState lock not held during 5s timeout
  - **B6**: `CodexSourceState.set_appserver_connected(bool)` → health `Healthy` (connected) / `Degraded` (capture fallback)
  - **C1**: Deleted `discover_appserver`, `poll_codex_appserver`, `CodexPollerConfig`, `--codex-appserver-addr` CLI option (5 legacy tests removed)
  - **Protocol fixes**: `result.data` (not `.threads`), `status.type` (object format), `updated_at` (not `lastUpdated`), thread/status/changed handles both object and string status
  - **Files**: `codex_poller.rs`, `poll_loop.rs`, `cli.rs`, `source.rs` (codex-appserver)
  - **Tests**: 594 total (net -3 from 597: -5 legacy + 1 split→2 + 1 health test)
  - `just verify` PASS

---

## 2026-02-26 (cont.)
### Current objective
- Phase 3b: Codex App Server 実働線の計画策定 (T-120, T-119)

### 計画内容

現状の Codex App Server → CLI パイプラインを調査し、以下の問題を特定:

**実働線が機能しない根本原因**: App Server から取得した thread event に `pane_id` が設定されない → daemon の `project_pane()` がスキップされる → Codex pane は poller heuristic のまま CLI に表示される。

**Protocol/Reliability bugs (T-120)**:
- B1: `"jsonrpc": "2.0"` フィールドが全メッセージに欠落 (仕様違反)
- B2: `initialized` notification に `"params": {}` 未設定
- B3: `used_appserver` フラグが events.is_empty() で判定 → idle 時に不要な capture fallback
- B4: App Server プロセス終了後の再接続なし
- B5: `poll_threads().await` 中に DaemonState mutex 保持 (5s timeout で全 API ブロック)
- B6: `codex_source` が常に Healthy を返す (App Server 死亡を検知不能)
- C1: legacy dead code (`discover_appserver`, `poll_codex_appserver`) が混乱の元

**Feature gap (T-119)**:
- `thread/list` response の `cwd` と tmux pane の `current_path` をマッチングし `pane_id` を付与
- `pane_generation` + `pane_birth_ts` も PaneGenerationTracker から取得して設定
- マッチング戦略: cwd 正規化比較、複数 pane は Codex process hint 優先、複数 thread は active 優先

**実装順序**: T-120 (protocol fix) → T-119 (pane correlation)
**Exit criteria**: `agtmux list-panes` で Codex pane が `signature_class: deterministic` 表示

### Docs updated
- `40_design.md`: pane_id correlation 設計追加、マッチング戦略記述
- `50_plan.md`: Phase 3b 追加
- `60_tasks.md`: T-120 新規、T-119 スコープ更新 (P2→P1、blocked_by T-120)

---

## 2026-02-26 (cont.)
### Current objective
- T-113a: Codex App Server integration (deterministic evidence from official API)

### Completed
- **T-113a**: Codex App Server integration: stdio client + capture fallback
  - **Primary path**: `CodexAppServerClient` in `codex_poller.rs`.
    - Spawns `codex app-server` as child process with stdio transport.
    - JSON-RPC 2.0 handshake: `initialize` → response → `initialized` notification.
    - `poll_threads()`: calls `thread/list` (limit=50, sorted by lastUpdated), emits events for status changes.
    - Notification translation: `turn/started` → `turn.started`, `turn/completed` → `turn.{status}`, `thread/status/changed` → `thread.{status}`.
    - Timeout: spawn 10s, poll 5s, notification drain 10ms.
    - Graceful degradation: if `codex` binary not found or handshake fails → `None`, capture fallback activates.
    - API reference: https://developers.openai.com/codex/app-server/
  - **Fallback path**: `parse_codex_capture_events()` + `CodexCaptureTracker`.
    - Parses NDJSON from tmux capture lines for `codex exec --json` output.
    - Content-based fingerprint dedup (`std::hash::DefaultHasher`) prevents re-ingestion across ticks.
    - `retain_panes()` cleans up departed pane tracking.
  - **poll_tick Step 6a integration**: tries app-server first (`poll_threads`), falls back to capture if app-server unavailable or returns no events.
  - **DaemonState additions**: `codex_appserver_client: Option<CodexAppServerClient>`, `codex_capture_tracker: CodexCaptureTracker`.
  - **tokio "process" feature** added to workspace Cargo.toml.
  - 12 new tests: 4 notification parsing, 1 app-server spawn graceful, 4 capture parsing, 3 poll_loop integration.
  - `just verify` PASS — 597 tests.

### Design note: Codex App Server API
The Codex App Server (https://developers.openai.com/codex/app-server/) provides JSON-RPC 2.0 over stdio/WebSocket:
- **Transport**: stdio (default, newline-delimited JSON) / WebSocket (experimental)
- **Handshake**: `initialize` → `initialized`
- **Key methods**: `thread/list`, `thread/read`, `turn/start`, `turn/interrupt`
- **Notifications**: `turn/started`, `turn/completed`, `thread/status/changed`, item events
- **Thread runtime status**: notLoaded, idle, systemError, active
- Future: WebSocket connection to external app-server for richer IDE integration.

### Gate evidence
- `just verify` PASS — 597 tests, 0 failures, fmt + clippy clean

### Next
- T-119: Codex App Server → pane_id correlation (thread.cwd ↔ tmux pane cwd matching)
- Waiting on user? yes — commit / 次のフェーズ決定

---

## 2026-02-26 (cont.)
### Current objective
- Phase 3: Post-MVP Hardening — Wire pure-logic crates into runtime (T-115〜T-118)

### Plan (Codex plan review: Go with changes, confidence Medium)

Implementation order: T-118 → T-116 → T-117 → T-115 ("observability first" + "lifecycle before admission")

| Task | Module | Key change | Tests |
|------|--------|------------|-------|
| T-118 | LatencyWindow | poll_tick SLO evaluation + `latency_status` API + path escaping fix | 5 |
| T-116 | CursorWatermarks | gateway cursor pipeline (advance_fetched/commit via watermarks) | 4 |
| T-117 | SourceRegistry | source.hello/heartbeat/staleness + list_source_registry API | 6 |
| T-115 | TrustGuard | UDS admission gate (warn-only) + daemon.info + source.ingest schema extension | 5 |

Codex review findings (all adopted):
- **F1 [Critical]**: source.ingest payload lacks source_id/nonce → schema extended with optional fields, fallback to source_kind
- **F2 [High]**: T-115 admission before T-117 registry → reordered (registry first)
- **F3 [High]**: Gateway 0-fallback → InvalidCursorTracker fires on runtime parse failure only (defensive)
- **F4 [High]**: evaluate(&mut self) → cache `last_latency_eval` in DaemonState, API returns cached value
- **F5 [Medium]**: path escaping only spaces → `shell_quote()` handles quotes/backslashes

### Completed
- **T-118**: LatencyWindow → poll tick metrics + path escaping fix (F2/F4/F5)
  - `DaemonState` に `latency_window: LatencyWindow` + `last_latency_eval: Option<LatencyEvaluation>` 追加。
  - poll_tick Step 12: `tick_start.elapsed()` → `record()` → `evaluate()` → breach/degraded logging → cached eval。
  - `latency_status` JSON-RPC method: cached `last_latency_eval` を返す (read-only, evaluate() を呼ばない)。
  - `shell_quote()`: 空白/引用符/バックスラッシュを含むパスを single-quote エスケープ。
  - 5 new tests (2 poll_loop latency, 1 server latency_status, 2 setup_hooks escaping)。

- **T-116**: CursorWatermarks → gateway cursor pipeline
  - `DaemonState` に `cursor_watermarks: CursorWatermarks` + `invalid_cursor_tracker: InvalidCursorTracker` 追加。
  - poll_tick Step 9a: gateway `next_cursor` → `parse_gw_cursor()` → `advance_fetched()` + `record_valid()`。NonMonotonic → `record_invalid()` → RetryFromCommitted/FullResync 回復。
  - poll_tick Step 11a: commit_cursor 前に `cursor_watermarks.commit()` で committed 追跡。
  - 4 new tests (advance, commit_equals_fetched, monotonic, caught_up)。

- **T-117**: SourceRegistry → connection lifecycle
  - `DaemonState` に `source_registry: SourceRegistry` 追加。
  - `source.hello` JSON-RPC: protocol version check → `handle_hello()` → Accepted/Rejected。
  - `source.heartbeat` JSON-RPC: `heartbeat(source_id, now_ms)` → `{acknowledged: bool}`。
  - `list_source_registry` JSON-RPC: serialized entries。
  - poll_tick Step 11b: `check_staleness(now_ms)` → stale source logging。
  - 6 new tests (hello accepted/rejected, heartbeat ack/unknown, staleness, list_registry)。

- **T-115**: TrustGuard → UDS admission gate (warn-only)
  - `DaemonState` に `trust_guard: TrustGuard` 追加。初期化: UID via `getuid()`, nonce=`{PID}-{nanos}`, 3 sources pre-registered (poller/codex_appserver/claude_hooks)。
  - `source.ingest` に warn-only admission gate 追加: `source_id`/`nonce` optional fields、未登録 or nonce 不一致 → `tracing::warn` のみ (Phase 1)。
  - `daemon.info` JSON-RPC method: nonce + version + pid。
  - `trust_guard.rs` に `nonce()`/`expected_uid()` accessor 追加。
  - 5 new tests (admits registered, warns unregistered, warns wrong nonce, daemon.info, pre-register 3)。

### Gate evidence
- `just verify` PASS — 585 tests, 0 failures, fmt + clippy clean
- Phase 3 Post-MVP Hardening **complete** (T-118 → T-116 → T-117 → T-115 全 4 タスク完了)

### Key decisions
- `getuid()` は `unsafe extern "C" { safe fn getuid() -> u32; }` (Rust 2024 edition) で直接呼び出し（libc crate 不要）。
- TrustGuard は Phase 1 = warn-only。Phase 2 (enforce) は後続タスク。
- `source.ingest` の `source_id`/`nonce` は optional — 未提供時は `source_kind` フォールバック + nonce check skip。

### Next
- Phase 3 完了。次のフェーズ: Persistence (SQLite), Multi-process extraction, TrustGuard enforce mode。
- Waiting on user? yes — commit / 次のフェーズ決定

---

## 2026-02-26 (cont.)
### Current objective
- T-111〜T-114: Deterministic source IO adapters + CLI title quality wiring

### Completed
- **T-111**: DaemonState 拡張 + deterministic source pipeline 配線
  - codex/claude source crate に `compact()` + `compact_offset` を追加（poller パターン移植）。
  - `DaemonState` に `codex_source: CodexSourceState`, `claude_source: ClaudeSourceState` 追加。
  - `poll_tick` に steps 8a/8b (codex/claude pull_events → gateway ingest) + compaction 追加。
  - Gateway を 3-source (`Poller`, `CodexAppserver`, `ClaudeHooks`) で初期化。
  - 11 new tests (6 source compact + 5 poll_loop integration)。
- **T-112**: UDS `source.ingest` エンドポイント + Claude hook adapter
  - `handle_connection` に `source.ingest` handler 追加（`claude_hooks`/`codex_appserver` dispatch、-32602 error handling）。
  - `scripts/agtmux-claude-hook.sh`: stdin JSON → jq 整形 → socat UDS 送信（fire-and-forget）。
  - `agtmux setup-hooks`: `.claude/settings.json` に 5 hook types (PreToolUse/PostToolUse/Notification/Stop/SubagentStop) を生成。
  - 9 new tests (4 UDS handler + 5 setup_hooks)。
- **T-113**: Codex appserver poller skeleton
  - `codex_poller.rs`: `discover_appserver()` (config override > env > well-known), `poll_codex_appserver()` (socket existence check, protocol TBD)。
  - `--codex-appserver-addr` CLI option (env: `CODEX_APPSERVER_ADDR`)。
  - 4 tests。Protocol 実装は Codex API ドキュメント確認後に調整。
- **T-114**: Deterministic session key 配線 + CLI title quality
  - `PaneRuntimeState` に `session_key: String` フィールド追加。
  - `build_pane_list()` で `evidence_mode == Deterministic` 時に `deterministic_session_key` を `TitleInput` に渡す → `DeterministicBinding` quality。
  - `build_summary_changed()` に `deterministic`/`heuristic` カウント追加。
  - 2 new tests。

### Review (Codex)
- 2 findings:
  - **F1 [High] REJECT (false positive)**: Claims `if let Ok(addr) = std::env::var(...) && !addr.is_empty()` doesn't compile — but this is valid Rust 2024 let chains syntax. `just verify` passes with 565 tests, confirming compilation.
  - **F2 [Medium] DEFER**: `generate_hooks_config` doesn't quote/escape script paths with spaces. Low risk for MVP (standard install paths don't contain spaces). Can address in post-MVP hardening.

### Gate evidence
- `just verify` PASS — 565 tests, 0 failures, fmt + clippy clean

### Key decisions
- Deterministic event timestamps must be fresh (< 3s) for resolver to accept them as `Fresh` tier — tests use `Utc::now()` instead of fixed timestamps.
- `handshake_confirmed` / `canonical_session_name` are Post-MVP (T-042 dependency + provider session file parser needed).
- Codex appserver protocol is a skeleton — discovery + socket check only, actual polling deferred until Codex API is documented.

### Learnings
- Rust 2024 edition makes `std::env::set_var`/`remove_var` unsafe — test code cannot manipulate env vars without unsafe blocks.
- `clap::Arg::env` requires the `"env"` feature flag on the clap dependency.

### Next
- T-111〜T-114 batch complete. All findings evaluated (1 rejected, 1 deferred).
- Waiting on user? yes — commit / next tasks

---

## 2026-02-26 (cont.)
### Current objective
- T-108: Runtime hardening batch (API completeness, memory compaction, SIGTERM)

### Completed
- **T-108a**: `list_panes` API に `signature_reason` + `signature_inputs` 追加 (FR-024 準拠)
  - `build_pane_list()` の managed pane JSON に `signature_reason` (string) と `signature_inputs` (object: provider_hint/cmd_match/poller_match/title_match) を追加。
  - 1 new test: `build_pane_list_includes_signature_fields`
- **T-108b**: Memory compaction — poller/gateway バッファの定期トリム
  - `PollerSourceState::compact(up_to_seq)`: absolute cursor → local index 変換で consumed events を drain。`compact_offset` で cursor 互換性維持。
  - `Gateway::compact_before(abs_position)` + `commit_cursor()` がバッファ compaction を実行。`compact_offset` で absolute cursor 維持。
  - poll_loop step 11: poller → gateway source cursor から poller compact、daemon gateway_cursor から gateway commit_cursor を毎 tick 実行。
  - 3 new poller tests: `compact_trims_consumed_events`, `compact_cursors_remain_valid`, `compact_beyond_buffer_is_safe`
  - 1 new gateway test: `compact_before_with_pagination` + `commit_cursor_compacts_buffer` (既存 noop テストを更新)
- **T-108c**: SIGTERM ハンドリング — `tokio::signal::unix::SignalKind::terminate()` を ctrl-c と並列で待機。`#[cfg(unix)]`/`#[cfg(not(unix))]` で cross-platform 対応。

- **T-109**: Title resolver wiring into `list_panes` API (FR-015/FR-016)
  - `resolve_title()` called in `build_pane_list()` for managed and unmanaged panes.
  - Managed panes: `TitleInput` with `provider`, `pane_title`, `is_managed=true` → HeuristicTitle quality (MVP: no deterministic sources wired, so canonical/handshake tiers are dormant).
  - Unmanaged panes: `TitleInput` with `pane_title`, `is_managed=false` → Unmanaged quality.
  - JSON response includes `title` (resolved string) and `title_quality` (tier name).
  - 1 new test: `build_pane_list_includes_resolved_title`
- **T-110**: Push event methods: `state_changed` + `summary_changed` (FR-010)
  - `state_changed`: accepts `since_version` param, returns version-based changes with pane state (signature_class, evidence_mode, activity_state, provider, confidence) and session state (presence, evidence_mode, activity_state, winner_tier). Uses daemon's `changes_since()` API.
  - `summary_changed`: accepts `since_version` param, returns `has_changes` flag, pane/session change counts, and summary (managed/unmanaged/total counts).
  - Both methods registered in UDS handler alongside existing list_panes/list_sessions/list_source_health.
  - 4 new tests: state_changed returns changes, state_changed no changes at current version, summary_changed returns counts, summary_changed no changes at current version.

### Review (Codex)
- 5 findings. Adoption:
  - **F1 [P0] ADOPT**: `compact(up_to_seq)` absolute→local conversion bug — 2nd+ compaction over-drained because `up_to_seq` was used as raw count instead of `up_to_seq - compact_offset`. Fixed with `saturating_sub(compact_offset)`.
  - **F2 [P1] ADOPT**: Gateway stale cursor `next_cursor` calculation — if `abs_start < compact_offset`, `next_pos = abs_start + returned_count` produced stale cursors causing re-delivery. Fixed with `abs_start.max(compact_offset)`.
  - **F3 [P1] DEFER**: `state_changed` missing signature_reason/inputs — `list_panes` already has these; push events are for change notification.
  - **F4 [P2] DEFER**: `summary_changed` managed/total from different data sources — practically harmless in single-process MVP.
  - **F5 [P3] DEFER**: SIGTERM `expect()` → Result — if SIGTERM registration fails, the process can't run anyway.
- 2 regression tests added: `compact_repeated_absolute_cursors_no_over_drain`, `stale_cursor_after_compaction_no_redelivery`

### Gate evidence
- `just verify` PASS — 538 tests (526 existing + 12 new), 0 failures, fmt + clippy clean

### Next
- Remaining MVP gaps: deterministic IO adapters (larger task, may need user input)
- Waiting on user? no

---

## 2026-02-26
### Current objective
- T-107: Detection accuracy + activity_state display (MVP)

### What changed (and why)
- **Capture-based detection (4th signal)**: `PaneMeta.capture_lines` + `ProviderDetectDef.capture_tokens` + `DetectResult.capture_match` を追加し、`detect()` で capture content をスキャンする第4シグナル (WEIGHT_POLLER_MATCH=0.78) を実装。
- **Capture tokens tightened (review adoption)**: `╭` → `╭ Claude Code` (lazygit/btop 等の TUI と衝突回避)、bare `codex` 削除 → `codex>` のみ (git log 等での偽陽性回避)。
- **Stale title suppression**: title_match のみ + shell cmd + no capture → `None`。Shell list: zsh/bash/fish/sh/dash/nu/pwsh/tcsh/csh。Case-insensitive + basename 抽出。
- **Per-pane activity_state + provider**: `PaneRuntimeState` に `activity_state: ActivityState` + `provider: Option<Provider>` 追加。`project_pane()` で投影。`changed` 条件に追加。
- **capture_match → poller_match 配線**: payload に `capture_match` を追加、`extract_signature_inputs()` で OR 合成。
- **list-panes 出力拡張**: `build_pane_list()` に `activity_state` + `provider` フィールド追加。
- **docs 更新**: 40_design.md (Detection Accuracy Hardening 更新)、60_tasks.md (T-107 DONE)。

### Review
- **Claude review (GO_WITH_CONDITIONS)**: 9 findings. High: capture token specificity (F1), payload data flow gap (F4). Adopted: F1, F2, F3, F4, F5, F7, F8. Deferred: F6 (provider hysteresis → post-MVP), F9 (capture_only_guard → depends on token specificity).
- **Codex review**: 6 findings. All aligned with Claude review. Extra finding: `changed` condition must include activity_state/provider — adopted.
- **Decision**: 2/2 GO (both reviewers completed). All High findings addressed in implementation.

### Evidence / Gates
- Tests: `just verify` PASS (525 tests = 514 existing + 11 new)
  - detect.rs: +9 tests (capture_match_claude, capture_match_codex, stale_title_shell_suppressed, stale_title_with_path_shell, stale_title_case_insensitive_shell, title_and_capture_corroborated, stale_title_not_suppressed_with_capture, cmd_basename_normalization, known_shells_list)
  - source.rs: +2 tests (poll_pane_capture_match_node_cmd, poll_pane_stale_title_shell_suppressed)
- Lint: `cargo clippy --all-targets` PASS
- Format: `cargo fmt --check` PASS

### Next
- E2E verification: `agtmux daemon` + `agtmux list-panes` で実環境確認
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v5 blueprint 用 docs を、テンプレ準拠の構造 (`00`〜`90`) で再編し、v4実装知見を反映する。

### What changed (and why)
- `docs/00_router.md` を作成し、docs-first運用契約を固定。
- `docs/10_foundation.md` と `docs/20_spec.md` を追加し、v5 の安定意図と可変要件を分離。
- 既存 `30/40/50` をテンプレ構造に合わせて再記述し、2層化・外部server・fallbackを実装可能粒度で定義。
- `60/70/80/85/90` を新設し、実行管理・判断記録・レビュー導線を整備。

### Evidence / Gates
- Context evidence:
  - v5 existing docs: `docs/30_architecture.md`, `docs/40_design.md`, `docs/50_plan.md`
  - v3 docs: `docs/v3/*`
  - v4 docs/code: `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/docs/v4/*`, `crates/*`
- Tests:
  - 未実行（本作業は docs 更新のみ）
- Typecheck:
  - 未実行
- Lint:
  - 未実行

### Learnings (repo-specific)
- Patterns:
  - v4 は `orchestrator.rs` に priority/fallback/health/dedup が集中。
  - source priority は実装済み（Claude: Hook>File>Poller、Codex: Api>Hook>File>Poller）。
  - source health freshness は `probe_interval + probe_timeout + 250ms` で判定。
- Pitfalls:
  - source ingest と snapshot refresh の同居により、責務境界とテスト境界が曖昧になりやすい。

### Next
- Next action:
  - Open Questions（Q-001〜Q-004）の回答を受けて tasks を確定し、T-010以降へ進む。
- Waiting on user? yes

---

## 2026-02-25
### Current objective
- ユーザー回答を仕様へ反映し、未決を縮小する。

### What changed (and why)
- poller 約85%は「v4時点の体感ベースライン」として再定義し、v5で再測定する方針へ更新。
- v5 MVP deterministic source を `Codex appserver` / `Claude hooks` で固定。
- gateway-daemon protocol を JSON-RPC over UDS で固定。
- `agents` 表記を英語固定で確定。
- 将来 capability 追加に備え、source server 拡張前提を architecture/design/tasks に追記。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー応答で上記4項目を確定
- Tests:
  - 未実行（docs 更新のみ）

### Learnings (repo-specific)
- 明示的な「固定事項」と「将来拡張余地」を分離して記述すると、実装フェーズで迷いが減る。

### Next
- Next action:
  - T-010（v5 crate skeleton）着手
  - T-033（poller baseline 再測定指標）を spec 化
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v4資産の再利用方針を実装計画へ組み込み、pane title 要件を固定する。

### What changed (and why)
- plan/tasks に v4再利用（poller/title/source-health）の明示タスクを追加。
- pane/session handshake 完了時に agent session name を優先表示する仕様を `spec/design` に追加。
- 該当方針を ADR に追記し、MVP固定事項として扱うようにした。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（v4再利用 + handshake title priority）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - T-010/T-011/T-012/T-013 の順で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- `managed/unmanaged` と `deterministic/heuristic` の語彙混線を解消し、命名規約を固定する。

### What changed (and why)
- `20_spec.md` に 2軸（presence / evidence mode）の命名規約を明示し、5カテゴリの推奨名と表示ラベルを追加。
- `30_architecture.md` の key flow を修正し、presence 判定と handshake による mode 昇格を分離。
- `40_design.md` の統合テスト観点を修正し、「managed化」と「deterministic昇格」を別ケース化。
- ADR に `managed/unmanaged` 固定定義と推奨 naming を追記。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（v4定義との整合、5カテゴリ命名の明確化）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - UI/API フィールド名（presence, evidence_mode）の実装時命名を T-050/T-060 で固定
- Waiting on user? no

---

## 2026-02-25
### Current objective
- Router を docs-first template 準拠に戻し、project固有記述の責務分離を明確化する。

### What changed (and why)
- `00_router.md` を process-only 契約へ再編し、subagent delegation / orchestrator ownership / plan mode policy / NEED_INFO loop を template 構成で明示した。
- `00_router.md` から仕様寄りの記述を排除し、意図・仕様は `10/20+` を正本とするルールを固定した。
- `60_tasks.md` のタイトルを template どおり `Orchestrator only` に更新した（内容は不変）。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（template準拠、Router責務の厳格化、subagent中心運用）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - `20+` を中心に実装可能粒度の記述を維持し、Routerへの逆流を防止する
- Waiting on user? no

---

## 2026-02-25
### Current objective
- local-first 開発フローを固定し、test/quality コマンドを `just` へ統一する。

### What changed (and why)
- `00_router.md` の Quality Gates を `just fmt` / `just lint` / `just test` / `just verify` に統一し、日次開発で commit/PR 非必須を明記。
- online/e2e source tests（codex/claude）に `just preflight-online` を必須化し、tmux/CLI auth/network 未準備時は fail-closed で中止する運用を追加。
- `20_spec.md` に FR-017 と DX/Constraint を追加し、preflight 要件と `justfile` 一元化を仕様へ昇格。
- `50_plan.md` と `60_tasks.md` を更新し、`justfile` 整備と source別テストスクリプト整備を明示タスク化。
- root `justfile` を新規追加し、`fmt/lint/test/verify/preflight-online/test-source-*` の実行入口を定義。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（git workflow 非依存の local 検証 + `just` 統一）
- Commands:
  - `just --list`（PASS）
- Tests:
  - `just verify` は未実行（workspace 実装前）

### Next
- Next action:
  - T-034 で `scripts/tests/test-source-*.sh` を実装し、preflight付き online/e2e を運用化
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v4を参照した online/e2e source tests を実装し、実行証跡を取得する。

### What changed (and why)
- `justfile` の preflight codex auth check を `codex login status` ベースへ修正し、現行CLI仕様と一致させた。
- `scripts/tests/test-source-codex.sh` / `test-source-claude.sh` / `test-source-poller.sh` を追加し、v4 wait=60（40s running / 120s idle）観測フローを shell で再現。
- claude では workspace trust gate の通過処理を追加し、無人実行で詰まらないようにした。
- test実行workspaceを `/tmp/agtmux-e2e-*` の隔離git repoへ切り替え、このrepoへ provider CLI session が紐づかないようにした。
- cleanup を強化し、各テスト終了時に tmux session/pane child process/temp workspace を自動削除するようにした。
- `60_tasks.md` の T-034 を DONE 化し、観測結果の差分（codexの120s内未確定）を注記した。

### Evidence / Gates
- Commands:
  - `just preflight-online`（PASS）
  - `just test-source-poller`（PASS: t+40s=`sleep`, t+120s=`zsh`）
  - `just test-source-codex`（PARTIAL: capture取得、`wait_result`未観測）
  - `just test-source-claude`（PASS: t+40s running, t+120s `wait_result=idle`）
- Tests:
  - online/e2e の基本実行導線は動作確認済み

### Next
- Next action:
  - codex ケースの prompt/観測窓を調整し、`wait_result`確定までの安定化を行う
- Waiting on user? no

---

## 2026-02-25
### Current objective
- provider model固定（claude/codex）と codex e2e 安定化を完了する。

### What changed (and why)
- claude e2e launch command を `--model claude-sonnet-4-6` 固定へ更新し、capture上で model marker を検証するようにした。
- codex e2e launch を interactive TUI から `codex exec --json`（v4 manifest 準拠）へ変更し、`--model gpt-5.3-codex` + `-c model_reasoning_effort=\"medium\"` を固定。
- codex は 40/120 より安定する 50/180 観測窓へ調整し、running時は pane process (`node/codex`)、idle時は `wait_result=idle` + `turn.completed` で判定するようにした。
- 既存の isolation/cleanup（tmp workspace, tmux session, child process cleanup）は維持。

### Evidence / Gates
- Commands:
  - `just preflight-online`（PASS）
  - `just test-source-codex`（PASS: model/effort marker, running@50s, idle marker@180s）
  - `just test-source-claude`（PASS: Sonnet 4.6 banner, running@40s, idle marker@120s）
- Post-check:
  - `tmux list-sessions | rg agtmux-e2e`（no residual sessions）
  - `/tmp/agtmux-e2e-*`（no residual workspaces）

### Next
- Next action:
  - codex/claude/poller の共通アサーションを script library 化して重複を削減する
- Waiting on user? no

---

## 2026-02-25
### Current objective
- e2e の連続信頼性（各agent 10回）と短縮/並列実行の成立性を確認する。

### What changed (and why)
- codex/claude script を `WAIT_SECONDS=30|60`、`PROMPT_STYLE=strict|compact`、agent別観測窓 override に対応させた。
- codex prompt は揺れ低減のため `wait_result=idle` 固定出力へ変更し、running 判定は pane process で担保する構成へ調整した。
- batch runner `scripts/tests/run-e2e-batch.sh` を追加し、codex/claude の並列反復実行と pass/fail 集計を自動化。
- matrix runner `scripts/tests/run-e2e-matrix.sh` を追加し、異なる時間窓/プロンプト（fast-compact / conservative-strict）を並列実行できるようにした。
- `justfile` に `test-e2e-batch` / `test-e2e-matrix` を追加。

### Evidence / Gates
- Commands:
  - `ITERATIONS=10 WAIT_SECONDS=30 PROMPT_STYLE=compact PARALLEL_AGENTS=1 AGENTS=codex,claude just test-e2e-batch`
    - codex: 10/10 pass
    - claude: 10/10 pass
    - total: 20/20 pass (100%)
  - `ITERATIONS_PER_CASE=2 PARALLEL_CASES=1 just test-e2e-matrix`
    - fast-compact: PASS
    - conservative-strict: PASS
- Post-check:
  - `tmux list-sessions | rg agtmux-e2e`（no residual sessions）
  - `/tmp/agtmux-e2e-(codex|claude|poller)-*`（no residual workspaces）
  - batch/matrix logs は `/tmp/agtmux-e2e-batch-*` / `/tmp/agtmux-e2e-matrix-*` に保持

### Next
- Next action:
  - 10x gate を nightly/手動 gate へ昇格し、失敗時は対応する iteration log を Review Pack に添付する
- Waiting on user? no

---

## 2026-02-25
### Current objective
- レビュー指摘3点（cursor契約 / binding state machine / 遅延予算）を docs 正本へ反映し、実装判断をなくす。

### What changed (and why)
- `20_spec.md` に FR-018〜FR-023 を追加し、ackベース cursor進行、safe rewind、pane-first identity、session representative pane、p95 2.0/5.0 を固定した。
- `30_architecture.md` に Flow-006/007 と storage/metrics 拡張を追加し、cursor replay safety と pane再利用対策をアーキ視点で明文化した。
- `40_design.md` に API契約（`heartbeat_ts`, `gateway.ack_delivery`, `invalid_cursor`）、data model（`pane_instance`/`binding_link`/`cursor_state`）、FSM、latency budget、テスト観点を追加した。
- `50_plan.md` と `60_tasks.md` を同期更新し、実装タスクを T-041/T-042/T-043 として分解した。
- `80_decisions/ADR-20260225-cursor-binding-latency.md` を新規追加し、代替案と採否理由を記録した。
- `90_index.md` を更新し、cursor/binding/latency の参照導線を追加した。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「docsを更新してください。これが正です。」「codingはしないでください。」）
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Next
- Next action:
  - T-040/T-041/T-042/T-043 を実装順で着手（gateway cursor -> binding FSM -> latency metrics）
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v4 と go-codex POC の実装実態を踏まえて、managed/unmanaged 判定を `pane signature v1` として docs 正本へ固定する。

### What changed (and why)
- v4（Rust）と exp/go-codex-implementation-poc（Go）を調査し、判定が env 固定ではなく `event/cmd/process/capture` 複合であることを確認した。
- `20_spec.md` に Pane Signature Model を追加し、FR-024〜FR-031（signature class/reason、重み、title-only guard、8s/45s/idle安定窓、no-agent連続2回）を固定した。
- `30_architecture.md` に pane signature classifier component と Flow-008（hysteresis guard）を追加した。
- `40_design.md` に signature contract/API fields、classifier アルゴリズム、error taxonomy、signature関連テスト観点を追加した。
- `50_plan.md` / `60_tasks.md` を同期し、T-044/T-045/T-046 を追加した。
- `80_decisions/ADR-20260225-pane-signature-v1.md` を新規追加し、代替案と採否理由を記録した。
- `90_index.md` に pane signature v1 の参照導線を追加した。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「それを踏まえたうえで、おすすめ」「その形でdocs更新」）
- Context evidence:
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux/.worktrees/exp/go-codex-implementation-poc`
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Next
- Next action:
  - T-044（signature classifier）-> T-045（hysteresis/no-agent）-> T-046（API露出）の順で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- `docs/v3` を撤去し、v5 blueprint docs のみを正本構成として維持する。

### What changed (and why)
- `docs/v3/*` を削除した。
- `90_index.md` の `v3/` 参照を削除し、現行ディレクトリ導線を v5 前提に揃えた。
- `70_progress.md` 既存履歴中の `docs/v3/*` 記述は過去時点の証跡として保持した（append-only ルール準拠）。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「docs下のv3は削除してよい」）
- Tests:
  - 未実行（本作業は docs 整理のみ）

### Next
- Next action:
  - v5 実装タスク（T-040 以降）を継続
- Waiting on user? no

---

## 2026-02-25
### Current objective
- review 指摘（poller gate / invalid_cursor / tombstone lifecycle / UDS trust / SLO運用 / backup-restore）を docs 正本へ固定する。

### What changed (and why)
- `20_spec.md` に FR-032〜FR-038 を追加し、poller受入基準、cursor数値契約、UDS trust admission、rolling SLO判定、snapshot/restore 契約を固定した。
- `30_architecture.md` に Flow-009/010 と `ops guardrail manager` を追加し、trust admission と運用復旧導線をアーキ構成へ反映した。
- `40_design.md` に `source.hello` 前提、UDS trust contract、checkpoint/rewind/streak、tombstone終端、SLO 3連続 breach 判定、Backup/Restore 設計、追加テスト観点を反映した。
- `50_plan.md` と `60_tasks.md` を同期更新し、T-047/T-048/T-049/T-051/T-071 を追加、T-033/T-041/T-042/T-043 を数値契約ベースに更新した。
- `90_index.md` を更新し、新契約への導線を追加した。
- `80_decisions/ADR-20260225-operational-guards.md` を追加し、運用ガードレールの採否理由を明文化した。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「では、docsを更新してください。」）
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Next
- Next action:
  - T-033（poller gate fixture固定）-> T-047（UDS trust）-> T-041（cursor recovery）の順で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- review 指摘（supervisor契約 / ack再送契約 / source registry lifecycle / ops guardrail実体 / Binding FSM並行制御）を docs 正本へ固定する。

### What changed (and why)
- `20_spec.md` に FR-039〜FR-047 を追加し、supervisor readiness+backoff+hold-down、delivery/ack 冪等契約、registry lifecycle、binding CAS、ops alert を固定した。
- `30_architecture.md` に Flow-011〜014 を追加し、起動再起動契約・ack redelivery・registry遷移・binding直列化をアーキフローへ反映した。
- `40_design.md` に `source.hello` contract、ack state machine、registry lifecycle、ops guardrail manager、binding concurrency control（single-writer + CAS）を具体化した。
- `50_plan.md` と `60_tasks.md` を同期し、T-052（supervisor contract）/T-053（binding concurrency）を追加、既存タスクの gate を retry/idempotency/lifecycle 前提へ更新した。
- `90_index.md` を更新し、新契約への参照導線を追加した。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「では、docsを改善してください。」）
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Next
- Next action:
  - T-041（ack/retry/idempotency）-> T-048（registry lifecycle）-> T-052（supervisor contract）の順で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- 実行方針を A（仕様駆動フル固定）から B（核心仕様 + 実装フィードバック）へ切り替え、実装開始可能な docs へ再編する。

### What changed (and why)
- `00_router.md` に `Execution Mode B` を追加し、Phase 1-2 は `[MVP]` 要件のみを実装ブロッカーに固定した。
- `20_spec.md` の FR-001〜FR-047 を `[MVP]` / `[Post-MVP]` にタグ分離した。
- `40_design.md` を `Main (MVP Slice)` と `Appendix (Post-MVP Hardening)` に再構成し、実装時に読む範囲を明確化した。
- `50_plan.md` を再編し、Phase 1-2=実装本線、Phase 3+=hardening backlog へ整理した。
- `60_tasks.md` を `MVP Track` / `Post-MVP Backlog` に分離し、全TODOへ `blocked_by` を追加して依存関係を明示した。
- `90_index.md` を `Start Here (MVP)` / `Hardening Later` 導線へ更新した。
- `80_decisions/ADR-20260225-core-first-mode-b.md` を追加し、方針転換の理由とガードレールを固定した。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「Bの方向性で書き換えてください」）
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Next
- Next action:
  - `MVP Track` の依存順に T-010 -> T-020 -> T-030/T-031/T-032 -> T-040 -> T-050 で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- 全 MVP タスク完了後のランタイム統合: pure logic crate を実際に動く CLI にする。

### What changed (and why)
- `20_spec.md` に MVP runtime policy を追加（single-process, spawn_blocking, in-memory, UDS 0700）。
- `30_architecture.md` に C-015（agtmux-tmux-v5）、C-016（agtmux-runtime）コンポーネントと Runtime Topology (MVP) を追加。
- `40_design.md` に Section 9「Runtime Integration (MVP)」を新設。tmux crate 設計、poll loop、cursor contract fix、memory management、UDS JSON-RPC server、CLI subcommands、signal handling、logging 仕様を固定。
- `50_plan.md` Phase 2 deliverables/exit criteria に runtime integration を追加。
- `60_tasks.md` に T-100〜T-106（runtime integration タスク群）を追加。
- `80_decisions/ADR-20260225-mvp-single-process-runtime.md` を新規作成。
- `90_index.md` に runtime integration 導線と ADR 参照を追加。
- Codex + Opus subagent の plan review を実施し、以下を採択:
  - (High) cursor re-delivery bug fix (T-100a)
  - (High) unmanaged pane visibility via synthetic events (T-103)
  - (High) 3-layer test strategy (T-106)
  - (Medium) memory compaction、signal handling、logging、socket security、pane generation tracking、v4 pattern reuse 等

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（「実際にCLIを動かせるところまで進めたい」「docsを正としたい」）
- Review:
  - Codex review: 6 findings (2 High, 4 Medium) → 全採択
  - Opus subagent review: 25 findings (3 High, 15 Medium, 7 Low) → High/Medium 全採択
- Tests:
  - 未実行（本作業は docs 更新のみ）

### Learnings (repo-specific)
- 既存 source の `next_cursor` は caught up 時に `None` を返す設計だが、gateway は `Some` 時のみ cursor 更新するため、runtime 統合時に re-delivery loop が発生する。T-100a で先行修正が必要。
- poller は非 agent pane に対してイベントを生成しないため、daemon は unmanaged pane を追跡できない。poll loop で synthetic event 生成が必要。
- 単一プロセスでも 1s polling × pane数 でメモリが単調増加するため、MVP でも最小 compaction が必須。

### Next
- Next action:
  - T-100 DONE（本セッション）→ T-100a cursor contract fix → T-101a tmux crate 着手
- Waiting on user? no

---

## 2026-02-26
### Current objective
- CLI 実稼働（T-100a ～ T-105 完了）

### Completed
- **T-100a**: cursor contract fix — 3 sources が caught up 時も `Some(current_pos)` を返すよう修正。Gateway は常に cursor を上書き。2 新テスト追加。471 tests pass.
- **T-101a**: `agtmux-tmux-v5` crate 新規作成 — TmuxCommandRunner trait (mock-injectable), TmuxExecutor (sync subprocess), tab-delimited list_panes parser, TmuxPaneInfo, TmuxError (thiserror). 10 parser unit tests.
- **T-101b**: capture_pane, inspect_pane_processes, PaneGenerationTracker, to_pane_snapshot. 13 tests.
- **T-102**: `agtmux-runtime` crate 新規作成 — `[[bin]] name="agtmux"`, clap derive CLI (daemon/status/list-panes/tmux-status), tracing + tracing-subscriber (AGTMUX_LOG env), signal handling (ctrl_c).
- **T-103**: poll loop — tmux → poller → gateway → daemon pipeline. unmanaged pane tracking via last_panes + build_pane_list merge. Error recovery (log+skip on capture failure).
- **T-104**: UDS JSON-RPC server — UnixListener (connection-per-request), socket dir 0700 + file 0600, stale socket detection. 3 methods: list_panes, list_sessions, list_source_health. Client CLI.
- **T-105**: CLI polish — tmux-status single-line output (`A:4 U:13`), socket targeting (--tmux-socket, AGTMUX_TMUX_SOCKET_PATH/NAME env), --poll-interval-ms.

### Key decisions
- Tab-delimited format string (instead of v4 colon-delimited) — avoids complex right-split parser for colons in pane_title.
- Unmanaged pane tracking via `last_panes` + `build_pane_list` merge (instead of synthetic events to daemon) — cleaner because daemon projection's resolver/tier logic doesn't apply to unmanaged panes.
- `default_socket_path()` uses `$XDG_RUNTIME_DIR` or `$USER` instead of libc getuid — avoids external dependency.

### Gate evidence
- `just verify` PASS — 498 tests, 0 failures, fmt + clippy (strict) + test.
- E2E smoke: `agtmux daemon` starts, polls 17 live tmux panes (4 agents, 13 unmanaged). `agtmux status` / `list-panes` / `tmux-status` all connect and display data.
- Workspace: 8 crates (6 lib + 1 tmux-io + 1 runtime bin).

### Learnings
- `gen` is a reserved keyword in Rust edition 2024 — must use `generation` or `r#gen`.
- `Arc<TmuxExecutor>` + `Arc::clone` for each `spawn_blocking` call is the clean pattern for sharing sync executors across async tasks.

### Next
- T-106 (P1) test strategy + quality gates for runtime crates
- Waiting on user? no

---

## 2026-02-26 (cont.)
### Current objective
- T-106: test strategy + quality gates for runtime crates

### Completed
- **T-106**: runtime test strategy implemented
  - Refactored `poll_tick`/`run_poll_loop` to generic `R: TmuxCommandRunner + 'static` (was concrete `TmuxExecutor`)
  - Created `FakeTmuxBackend` implementing `TmuxCommandRunner` with configurable list-panes output, per-pane capture data, error injection
  - 12 integration tests in poll_loop.rs: claude/codex agent detection, unmanaged pane tracking, mixed agents+unmanaged, empty tmux, list-panes failure, capture failure recovery, gateway cursor, no-redelivery, generation tracker, large batch (20 panes), multiple sessions
  - 4 unit tests in server.rs for `build_pane_list`: empty state, all unmanaged, managed+unmanaged merge, no-duplicate for managed pane
  - E2E smoke script: `scripts/tests/test-e2e-status.sh` (start daemon → wait socket → run status → verify output + tmux-status pattern)
  - justfile: `test-e2e-status`, `run-daemon`, `run-status` recipes added

### Gate evidence
- `just verify` PASS — 514 tests (up from 498), 0 failures
- `just test-e2e-status` PASS — daemon starts, status returns `Panes: 17 total (4 agents, 13 unmanaged)`, tmux-status returns `A:4 U:13`

### Learnings
- `PanePresence::Unmanaged` serializes to lowercase `"unmanaged"` (serde default), not `"Unmanaged"`
- UDS server `set_permissions` on socket parent dir fails if parent is `/tmp/` (no ownership) — E2E test socket path must include a dedicated subdirectory

### Summary
- All MVP tasks (T-100 through T-106) complete. CLI runs, 514 tests pass, E2E smoke verified.
- Waiting on user? yes — next steps (post-MVP hardening, persistence, multi-process extraction)

---

## 2026-02-26 (cont.)
### Current objective
- T-121: Pane-first resolver grouping — evidence_mode ダウングレード防止

### Investigation
- **バグ再現**: Codex pane で deterministic evidence (AppServer) があるのに、Claude の deferred (heuristic/poller) evidence が優先される現象を調査
- **根本原因**: `apply_events()` が `session_key` でイベントをグループ化するが、各 source は異なる `session_key` を使用する:
  - Poller: `"poller-{pane_id}"` (Heuristic)
  - Codex AppServer: `thread_id` (Deterministic)
  - Claude Hooks: `session_id` (Deterministic)
- 同一 pane のイベントが別々の resolver セッションで処理され、`project_pane()` の last-writer-wins で Heuristic が Deterministic を上書きできる
- **代替案検討**: PaneTierArbiter (二段解決) を検討したが、Codex→Claude 切替時に Codex AppServer が thread/list events を出し続け `det_last_seen` を fresh に保つため、Claude heuristic が永久にブロックされる致命的欠陥を発見

### Design decision
- **Pane-first grouping**: `apply_events()` のグループ化キーを `session_key` → `pane_id` (fallback: `session_to_pane` → `session_key`) に変更
- 同一 pane の全ソースイベントが同一 resolver batch に入り、既存の tier 抑制 + rank 抑制がそのまま正しく機能する
- **核心不変条件**: 同一 pane の全ソースイベントが同一 resolver batch で処理される
- 変更対象: `projection.rs` のみ (4 modification points)
- resolver.rs は pure function — グループ化は呼び出し側の責務、resolver 変更不要

### Reproduction tests (9 tests written, 3 FAIL = bug confirmed)
- `cross_session_det_overwritten_by_heur_sequential_ticks` — **FAIL**: fresh Det(1s) が Heur に上書き
- `cross_session_claude_det_plus_poller_heur` — **FAIL**: Claude Det が Poller Heur に上書き
- `deterministic_fresh_active_cross_session` — **FAIL**: per-session freshness で demotion 誤発火
- 他 6 テスト PASS (edge cases: stale takeover, recovery, provider switch, 3-source, pane_id=None fallback)

### Docs updated
- `20_spec.md`: FR-031a 追加 (pane-first grouping 必須)
- `30_architecture.md`: Flow-003 に pane-first grouping 注記追加
- `40_design.md`: Section 3 (Resolver and Arbitration) に pane-first grouping 設計追加、Poll Loop Step 10 に投影詳細追加
- `60_tasks.md`: T-121 DOING 追加
- `70_progress.md`: 本エントリ

### Key decisions
- `session_key` 単位のグループ化は cross-source tier 抑制が機能しないため禁止 (FR-031a)
- `pane_id` なしイベントは `session_to_pane` HashMap で fallback し、それもない場合は `session_key` を使用
- Provider 切り替え時 (Codex→Claude) は 3s freshness window で自然に切り替わる — 旧 provider の deterministic イベントが停止すると stale → heuristic takeover

### Learnings
- Resolver は pure function で正しく設計されている — バグは呼び出し側のグループ化にあった
- 同一 pane に対して複数 source が異なる `session_key` を使う構造は、pane-first grouping で解決が最もシンプル
