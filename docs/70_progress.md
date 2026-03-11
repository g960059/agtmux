# Progress Ledger (append-only)

## Rules
- Append only. 既存履歴は書き換えない。
- 記録対象: 仕様変更、判断、ユーザー要望、学び、gate証跡。

---

## 2026-03-11 — Phase 10: Session Metadata Display plan finalized

### Design Decision (synthesized from Codex + Claude independent proposals)
- **Option A** (single `session_subtitle` field) chosen unanimously
- **De-duplication on daemon side**: subtitle = first candidate from (summary, first_prompt) that differs from conversation_title
- **2-line sidebar row** for managed panes only: title (13pt bold) + subtitle (11pt, opacity 0.45), fixed row height
- Research doc: `docs/research/20260311/session-metadata-display.md`
- Tasks: T-SM01 (daemon) + T-SM02 (agtmux-term), both added to docs/60_tasks.md
- Handoff: `agtmux-term/docs/handoffs/T-SM02-session-subtitle.md`

### Status
- T-SM01 implementation delegated to Codex (background)

---

## 2026-03-11 — T-SV2 deletion chain + T-term01 + P3 follow-ups (session closeout)

### What landed
- T-SV2-P2: removed `ui.bootstrap.v2` / `ui.changes.v2` RPC endpoints from daemon (commit `9e4fb9e`)
- T-SV2-P3: deleted `agtmux-core-v5::sync_v2_compat` module (commit `02ffdd5`)
- agtmux-term T-150/T-151: removed term-side sync-v2 compat layer (pushed `288238f` via Codex %2)
- T-term01 (agtmux-term): hook setup status check + Register/Unregister sidebar UI (commit `288238f`)
  - `HookSetupStatus` enum, `@Published hookSetupStatus`, startup check, `HookWarningBanner` in SidebarView
- T-D01–T-D05: confirmed all Phase 7 distribution infra already present (no new code needed)
- T-E03a: `check_hooks_at_path()` refactor + filesystem integration test
- T-E03b: `poll_tick_session_start_populates_transcript_path_hint` test in poll_loop
- T-codex03a: explicit `sleep 4` in test-claude-approval.sh Phase 3
- T-E05a: closed as structural guarantee (claude signals have no WaitingInput patterns by design)

### Gate
- `just verify` PASS (agtmux, final commit `030732b`)
- `swift build` + `swift test` 296/296 deterministic PASS (agtmux-term `288238f`)
- T-XTERM-A2/A3/A4/A5/A6 all DONE; T-SV2-P1/P2/P3 all DONE

### Open backlog
- T-E04 (Post-MVP): OSC Tap source — new crate, deferred
- T-codex03b (P3): codex-title.sh online re-verification — needs live Codex session

## 2026-03-10 — Codex exec parity landed on the deterministic JSONL path

### Summary

- `codex exec --json` / `codex --yolo` can now reach reducer-backed sync-v3 `primary=.running` without reviving the removed appserver path.
- The runtime keeps `CodexJsonl` as the only deterministic Codex semantic source:
  - interactive transcript discovery still comes from `CODEX_HOME` / `HOME/.codex/sessions`
  - exec mode now writes a pane-bound synthetic spool JSONL and binds that exact file to the exact pane
- term-side strict Codex running proof moved to exec parity, while one interactive sentinel remains to protect the persistent transcript delivery path separately.

### What landed

- `crates/agtmux-runtime/src/codex_exec_spool.rs`
  - normalizes exec NDJSON into transcript-like JSONL lines (`task_started`, `function_call`, `function_call_output`, `task_complete`)
  - writes `session_meta` once and emits a pane-bound discovery hint with `session_key_override = codex:%pane`
- `poll_loop.rs`
  - captures joined pane output for Codex candidates
  - keeps a pane-local spool tracker keyed by exact pane identity
  - routes spool-backed deterministic hints into existing `CodexJsonl` discovery
- `discovery.rs`
  - explicit spool path binding now wins over same-CWD discovery
  - missing explicit spool path no longer falls back to sibling same-CWD panes

### Gate

- `cargo fmt --all`
- `cargo test -p agtmux-source-codex-jsonl`
- `cargo test -p agtmux-tmux-v5`
- `cargo test -p agtmux`
- focused proofs:
  - `cargo test -p agtmux codex_exec_spool -- --nocapture`
  - `cargo test -p agtmux poll_tick_exec_json_promotes_exact_pane_to_sync_v3_running_without_same_cwd_bleed -- --nocapture`
  - `cargo test -p agtmux poll_tick_exec_spool_rehydrates_running_truth_after_restart -- --nocapture`

### Contract note

- Current Codex deterministic truth is now:
  - interactive transcript path via real `~/.codex/sessions/**/*.jsonl`
  - exec parity path via pane-bound synthetic spool JSONL
- `poller` remains attribution-only for Codex.
- Interactive launch remains as a narrow sentinel in `agtmux-term`; it is no longer the only strict running-state live proof.

## 2026-03-09 — Codex strict running-state contradiction: source discovery bug isolated and fixed

### Summary

- Term-like live repro no longer pointed at timing. Interactive Codex runs produced exact-row `provider=codex presence=managed`, but `PanePresentationState` stayed `inactive`.
- Repo-owned 2-pane proof then showed the missing seam directly:
  - `ui.bootstrap.v3` for the exact Codex pane stayed `thread.lifecycle=not_loaded`
  - `list_source_health` kept `codex_jsonl=status=down` for the entire run
  - the matched transcript under `~/.codex/sessions/...jsonl` already contained `task_started`, `user_message`, `function_call`, `function_call_output`, and `task_complete`
- Conclusion: this was not a Codex FSM / reducer mapping bug. The reducer never came alive because discovery never scanned the real transcript directory when `CODEX_HOME` was unset.

### Root Cause

- `crates/agtmux-source-codex-jsonl/src/discovery.rs`
  - `codex_home_dir_from_env()` returned `HOME` instead of `HOME/.codex`
  - downstream `discover_sessions()` therefore scanned `~/sessions`, while real Codex CLI transcripts lived in `~/.codex/sessions`
- That left sync-v3 on the managed fallback row:
  - `provider=codex`
  - `presence=managed`
  - `thread.lifecycle=not_loaded`
  - presentation `primary=inactive`

### Fix

- Fallback path changed from `HOME` to `HOME/.codex`
- Added focused source/runtime coverage:
  - HOME-only env resolves to `~/.codex`
  - poll tick discovers Codex JSONL from `HOME/.codex/sessions` without `CODEX_HOME`
  - resulting sync-v3 bootstrap row becomes reducer-backed `thread.lifecycle=active`

### Gate

- `cargo fmt --all`
- `cargo test -p agtmux-source-codex-jsonl codex_home_dir_falls_back_to_home_env -- --nocapture`
- `cargo test -p agtmux poll_tick_discovers_codex_jsonl_via_home_dot_codex_fallback -- --nocapture`
- `cargo test -p agtmux poll_tick_discovers_codex_jsonl_from_node_runtime_without_process_hint -- --nocapture`
- `cargo test -p agtmux-source-codex-jsonl discover_sessions_finds_matching_jsonl -- --nocapture`
- repo-owned term-like live rerun with fresh `target/debug/agtmux`: exact Codex row reached `provider=codex presence=managed thread.lifecycle=active`, and `codex_jsonl` source health flipped `down -> healthy` on the same path

---

## 2026-03-03 — Phase 9 完了: Codex JSONL セマンティックソース 実装

### T-codex01a/b/c 完了 (2026-03-03)

**概要**: 4エージェント調査に基づき、Codex 検出を mtime ヒューリスティックから JSONL セマンティック解析に根本置換。

**T-codex01a — 回帰テスト作成 (TDD)**
- `crates/agtmux-source-codex-jsonl/src/fsm.rs`: 22 unit tests
- `crates/agtmux-source-codex-jsonl/src/watcher.rs`: 4 unit tests
- `crates/agtmux-source-codex-jsonl/src/discovery.rs`: 6 unit tests
- `crates/agtmux-source-codex-jsonl/src/translate.rs`: 7 unit tests
- `crates/agtmux-source-codex-jsonl/src/source.rs`: 7 unit tests + cursor pagination
- `scripts/tests/e2e/scenarios/codex-semantic-states.sh`: synthetic JSONL injection e2e

**T-codex01b — 新クレート実装**
- `crates/agtmux-source-codex-jsonl/` 新規作成（5 モジュール）
- `SourceKind::CodexJsonl` を `agtmux-core-v5/src/types.rs` に追加
- `discovery.rs`: `lsof -p <pid> -d cwd -Fn` で CWD 取得 → `~/.codex/sessions/**/*.jsonl` 全走査 → `session_meta.payload.cwd` マッチ
- `fsm.rs`: 6状態 FSM — `.payload.type` キー使用（旧実装の `.data.type` キーバグを修正）
- 正しいイベント名: `task_started`, `task_complete`, `entered_review_mode`, `exited_review_mode`, `function_call`, `function_call_output`
- Gate: `just verify` 834+ tests PASS

**T-codex01c — poll_loop 接続 + 旧コード削除**
- `codex_poller.rs`: 700行 → 4行スタブ（`CodexAppServerClient`, `scan_jsonl_sessions` Pass1/2/3, `CodexCaptureTracker` 等 削除）
- `poll_loop.rs` Step 6a: App Server 180行ブロック → Codex JSONL source 50行に置換
- `DaemonState` から削除: `codex_appserver_client`, `codex_appserver_had_connection`, `codex_supervisor`, `codex_capture_tracker`, `codex_source`
- `DaemonState` に追加: `codex_jsonl_source: CodexJsonlSourceState`, `codex_jsonl_watchers`
- `server.rs`: `codex_appserver` ソース kind 削除
- Gateway: `SourceKind::CodexAppserver` → `SourceKind::CodexJsonl` 置換
- `agtmux-runtime/Cargo.toml`: `agtmux-source-codex-appserver` → `agtmux-source-codex-jsonl`
- Gate: `just verify` 800+ tests PASS, zero warnings, zero clippy

---

## 2026-03-02 — Phase 9 設計: Codex JSONL セマンティックソース 調査完了

### 4 エージェント研究チーム調査 (2026-03-02)

**背景**: v0.1.9〜v0.1.12 にわたる Codex 検出バグを根本解決するための調査。
詳細: `docs/research/20260302/05_synthesis.md`

**チーム構成**:
- Agent A (Claude Opus): JSONL スキーマ詳細 + Proposal A
- Agent B (Claude Opus): ESC シーケンス + cmux 分析 + Proposal B
- Codex C (gpt-5.3): 1130 ファイルから実データ分析
- Codex D (gpt-5.3): ESC + tmux アーキテクチャ調査

### 重要発見（設計方針への反映）

**1. JSONL JSON キーが違った**: `.data.type` ではなく `.payload.type`

**2. 正しいイベント名**:

| 旧仮定 | 実際のイベント（1130ファイルより） |
|--------|-----------------------------------|
| `turn/started` | `task_started` (1785件) |
| `turn/completed` | `task_complete` (1561件) |
| `waitingOnApproval` | `entered_review_mode` (46件) / `exited_review_mode` (46件) |

**3. keepalive 行は存在しない**: Idle 時はファイル書き込みが停止するのみ。v0.1.11/v0.1.12 で戦っていた問題は存在しなかった。

**4. WaitingApproval はヒューリスティック不要**: `entered_review_mode` が明示的に存在するため確定検出可能。

**5. complete イベント名**: `function_call` / `function_call_output` (各 42968 件) が ToolExecuting の遷移シグナル。

**6. cmux**: libghostty ベースの native macOS ターミナル。tmux から外部観測する agtmux とはアーキテクチャが異なる。OSC シーケンスは Phase 2（Post-MVP）で対応。

### 確定 FSM

```
Init → (task_started) → Running
Running → (function_call) → ToolExecuting → (function_call_output) → Running
Running → (entered_review_mode) → WaitingApproval → (exited_review_mode) → Running
Running/ToolExecuting → (task_complete / turn_aborted) → WaitingInput
WaitingInput → (task_started) → Running
WaitingInput → (process exit or 600s) → Ended
```

### 実装方針

Phase 1（今すぐ）: `agtmux-source-codex-jsonl` 新クレート（T-codex01a/b/c）
- TDD: 回帰テスト + e2e テスト先行
- App Server, scan_jsonl_sessions, mtime コードを完全削除
Phase 2（Post-MVP）: OSC 9;4 tap via pipe-pane（T-E04 継続）

---

## 2026-03-02 — CI/CD 安定化 + Codex historical enrichment 改善

### v0.1.4 — 既存デーモン自動置換

**背景**: 既存デーモンが動いている状態で `agtmux daemon` を起動すると即座にエラーで終了していた。
ユーザーは kill を忘れる → 新デーモンが立ち上がらない。

**修正**: `crates/agtmux-runtime/src/server.rs`
- ソケットに接続できる場合: `daemon.info` JSON-RPC で PID を取得 → SIGTERM → ソケット消滅待ち (3秒) → force remove
- ソケットが stale な場合: そのまま削除して起動

**Gate**: `just verify` PASS → v0.1.4 タグ push → CI/CD PASS、Homebrew tap 更新

---

### v0.1.5 — Codex historical JSONL enrichment

**背景**: Codex JSONL scanner がファイル mtime ≤ 120秒のもの (`JSONL_IDLE_THRESHOLD_SECS`) しか処理しないため、
デーモン再起動後に既存 Codex セッション (数時間前のファイル) が evidence なしのまま heuristic に落ちていた。
結果: `evidence_mode=heuristic`, `updated_at=just now`, `conversation_title=null`, `activity_state=Unknown`

**修正**: `crates/agtmux-runtime/src/codex_poller.rs`
1. `scan_jsonl_sessions()` 末尾に historical enrichment pass を追加 (7日分スキャン)
2. 新規フィールド `actual_activity_at: Option<DateTime<Utc>>` を `CodexRawEvent` に追加
3. projection が `actual_activity_at` を `updated_at` として使用 → ファイル mtime が正しく反映される
4. `is_heartbeat = false` を明示 → projection が `updated_at` を上書きするトリガーになる

**Gate**: `just verify` PASS → v0.1.5 タグ push → CI **FAIL** (下記参照)

---

### 2026-03-02 — CI/CD 根本修正 (v0.1.5 失敗の原因と対策)

**失敗原因**: ローカル `lint` と CI `clippy` のフラグが不一致
- CI: `cargo clippy --workspace -- -D warnings` → すべての warning がエラー
- ローカル: `cargo clippy --workspace --all-targets --all-features --locked -- -D clippy::dbg_macro ...` → `-D warnings` なし

このため `clippy::unnecessary_map_or` lint (`map_or(true, ...)` → `is_none_or(...)`) がローカルでは通り CI で失敗した。

**修正内容**:
1. `crates/agtmux-runtime/src/codex_poller.rs:444` — `map_or(true, ...)` → `is_none_or(...)`
2. `crates/agtmux-runtime/src/codex_poller.rs:400` — historical enrichment pass の条件を tier ベースに改善
   - `if pane.process_hint.as_deref() != Some("codex")` → `if pane_tier(pane) > 1`
   - tier 0 (`Some("codex")`) と tier 1 (`None` = neutral runtime like node) を許可、競合エージェントとシェルを除外
3. `justfile` — `lint` を `cargo clippy --workspace -- -D warnings -D clippy::...` に更新 (CI と一致)
4. `scripts/pre-commit.sh` + `.git/hooks/pre-commit` — `cargo fmt` に加え `cargo clippy -- -D warnings` を追加
5. `justfile` — `install-hooks` ターゲット追加 (クローン後 `just install-hooks` で一発設定)

**教訓**: ローカル lint と CI lint は常に同一フラグであること。差異があると「ローカル PASS、CI FAIL」のサイクルに陥る。

**Gate**: `just verify` PASS (771 tests, 0 failed)

---

## 2026-03-01 — T-135c Claude summary + sessions-index.json フォールバック DONE

### 実装内容

`/resume` 一覧で表示される Claude の会話タイトルを最大活用するため、2 つの追加ソースを実装。

**調査結果**: Claude JSONL には 3 種類のタイトルソースが存在する。
1. `custom-title` イベント (`/rename` コマンド) — T-135b で実装済み
2. `summary` イベント (AI が自動生成、セッション終了時) — **T-135c で追加**
3. `sessions-index.json` (`summary` + `firstPrompt` フィールド) — **T-135c で追加**

**Priority chain**: `custom-title > summary(watcher) > summary(sessions-index) > firstPrompt(sessions-index)`

`summary` イベントは JSONL ファイル末尾に書かれるため、watcher が EOF から開始するデーモン再起動後は
検出できない。`sessions-index.json` フォールバックがこのギャップをカバーする。

### 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `translate.rs` | `summary: Option<String>` フィールド追加 |
| `watcher.rs` | `last_summary` + getter/setter 追加 |
| `source.rs` | `type=summary` ハンドラ追加（2 unit tests）|
| `discovery.rs` | `SessionIndexEntry` に `summary`/`first_prompt` + `read_session_index_entry()` pub fn（2 unit tests）|
| `poll_loop.rs` | 3段階優先チェーン（summary→custom-title→sessions-index fallback）|
| `lib.rs` | `pub use discovery::read_session_index_entry` 追加 |
| `scenarios/claude-summary.sh` | 新規 e2e シナリオ (Phase 3: summary→title, Phase 4: custom-title wins) |
| `online/run-all.sh` | `claude-title` + `claude-summary` + `codex-title` シナリオ追加 |

### Gate証跡

- `just verify`: PASS（6 新規 unit tests、合計 759 tests）
- `PROVIDER=claude e2e-online`: 5 passed, 0 failed
  - `single-agent-lifecycle` PASS
  - `multi-agent-same-session` PASS
  - `provider-switch` PASS
  - `claude-title` PASS（regression）
  - `claude-summary` PASS（新規）

---

## 2026-02-28 — Phase 7 Distribution 戦略決定

### 背景

Phase 6 CLI 再設計（T-139〜T-140）が完了し、CLI として使える状態になった。
次のステップとして、AIツールが乱立する現市場ではインストールの容易さが採用の必須条件と判断し、
配布戦略を序盤から固めることにした。

Claude Orchestrator / Claude subagent / Codex (gpt-5.3-codex) の3者による独立した案を競合させ、
以下の統合方針を決定した。

### 決定: 配布チャネル

| チャネル | 判断 | 理由 |
|---------|------|------|
| Homebrew tap (macOS) | **Primary** | ターゲット層の最短導線、`brew upgrade` でアップデート |
| curl installer + musl binary (Linux) | **Secondary** | glibc 依存ゼロで全ディストロ対応 |
| cargo install (Rust ユーザー) | **Tertiary** | crates.io publish で信頼感向上 |
| homebrew/core | **Long-term** | ~75 stars + 30日公開実績で申請判断 |
| Windows / winget / scoop | **Scope-out** | tmux 非対応のため明示的に除外 |

### 決定: 設計上の制約

- `self-update` は実装しない（Homebrew との衝突回避、Codex 提案採用）
- `agtmux --version` は tmux なしでも成功すること（Homebrew `test do` の前提、Claude subagent 提案採用）
- musl static binary で Linux 対応（Claude subagent 提案採用）
- Artifact Attestation を初日から有効化（Codex 提案採用）

### 決定: ツールチェーン

- `cargo-dist`（axodotdev）を採用: Homebrew tap 自動更新・install.sh 生成・GitHub Actions 生成を一括カバー
- musl cross-compile は `cross` クレートで Docker ベースビルドに統一

### 詳細

`docs/55_distribution.md` を新規作成し、完全な戦略を記録。

---

## 2026-02-27 — Phase 4/5 スコープ決定 + Phase 6 CLI/TUI 方針

### 背景
T-128 完了 (675 tests) を機に、今後の方向性を整理した。v4 が production に進んでおらずユーザーもいないため、migration strategy は不要と判断。daemon infrastructure は実質完成しており、次フェーズはユーザーが実際に触れる CLI/TUI の構築とした。

### 決定: Phase 4 スコープ縮小

| 項目 | 決定 | 理由 |
|------|------|------|
| Supervisor strict (T-129) | **実施** | Codex crash storm 防止。純ロジックは実装済み、poll_loop.rs への wiring のみ |
| TrustGuard enforce | **DROPPED** | 個人利用 + 単一ユーザー環境。warn-only で十分。複数ユーザー環境のニーズが生じた時点で再検討 |
| Persistence (SQLite) | **DROPPED** | daemon の自然回復は 2〜4 秒。tmux の pane_id は tmux server 再起動で変わるため長期保存データが有害になりうる。max_age_ms=10min があっても根本的解決にならない |
| Multi-process extraction | **DROPPED** | GUI バンドル版は single-process で十分。分離のニーズが生じた時点で検討 |
| ops guardrail manager | **DROPPED** | 運用規模が小さい間は不要 |

### 決定: Phase 5 Migration — DROPPED
v4 は production に進んでおらず、切り替え戦略は不要。runbook は参照用に docs に残るが、タスクとして追跡しない。

### 決定: Phase 6 CLI/TUI — 次フェーズとして開始
tcmux (https://github.com/k1LoW/tcmux) を参考に、daemon を backend とした精密版 CLI を構築する。

**agtmux が tcmux より優れる点**:
- `activity_state: Running / Idle / Waiting` が deterministic sources から取得可能（tcmux はプロセス検出のみ）
- `evidence_mode: Deterministic / Heuristic` で検出根拠を明示
- 複数 agent 並列（Codex + Claude 同一 window）を正確に区別

**実装順序**: T-129 (Supervisor strict) → T-130 (API field 追加) → T-131 (list-windows) → T-132 (fzf recipe)

**build_pane_list の不足フィールド確認** (T-130):
- `window_id` (@N), `session_id` ($N), `current_path` — `TmuxPaneInfo` には存在するが `build_pane_list` で未露出
- 追加のみで API 後方互換を維持可能

---

## 2026-02-27 — T-129: Supervisor strict wiring — Completed

### 変更内容
- `DaemonState.codex_reconnect_failures: u32` を廃止し `codex_supervisor: SupervisorTracker` に置き換え
- 接続試行判断:
  - `Ready` → 即時試行
  - `Restarting { next_restart_ms }` → `now_ms >= next_restart_ms` になったら試行
  - `HoldDown { until_ms }` → 期限前は `debug!` ログのみでスキップ、期限後は試行再開
- 成功時: `record_success()` → Ready にリセット
- 失敗時: `record_failure(now_ms)`:
  - `Restart { after_ms }` → `info!` ログ (1s→2s→4s→…→30s)
  - `HoldDown { duration_ms }` → `warn!` ログ (5回/10min超過→5min停止)

### 旧実装との違い
旧: カウンタベース (`backoff_ticks = 2^failures` tick を skip)
- 問題: tick 数ベースなので poll_interval 変更で挙動が変わる
- 問題: failure budget なし → crash storm 時に無限リトライ

新: 時刻ベース (next_restart_ms, until_ms で判断)
- poll_interval に依存しない
- failure_budget=5/10min + holddown_ms=300s → crash storm を自動抑制

### テスト (4 new)
1. `supervisor_initial_state_is_ready` — `DaemonState::new()` で Ready
2. `supervisor_failure_advances_to_restarting` — 1回失敗 → Restarting + after_ms=1000
3. `supervisor_success_after_failure_resets_to_ready` — 成功で Ready に戻る
4. `supervisor_budget_exhaustion_triggers_holddown` — 5回失敗 → HoldDown 300s

### Gate evidence
679 tests total (675 → 679), `just verify` PASS (fmt + lint + test)

---

## 2026-02-27 (cont.)
### T-127: Pane attribution false-positive fixes — Design Decision

#### 3 bugs identified via live `agtmux list-panes`
1. **Bug A**: `%35` (Codex/node, CWD=test-session) → `claude deterministic` (should be `codex`)
   - Root cause: T-126 Phase 3 で `bootstrap_event(is_heartbeat=false)` を全 CWD 候補に emit。`%35` と `%297` が同一 CWD を持つため、両方に `last_real_activity[Claude]` が書き込まれる。`select_winning_provider` で T_claude ≥ T_codex → Claude wins 誤判定
2. **Bug B**: `%391` (yazi file manager) → `claude deterministic` (should be `unmanaged`)
   - Root cause: Step 6b フィルタが `Some("shell") | Some("codex")` のみ除外。`process_hint=None` (yazi) はフィルタを通過してしまう
3. **Bug C**: `%287`, `%307` (zsh) → `claude heuristic` (should be `unmanaged`)
   - Root cause: `detect()` が `process_hint=Some("shell")` をチェックしない。terminal capture に Claude-like テキストがあると誤判定

#### 3 architectural approaches compared

**Option 1 (Claude agent)**: `DeterministicClaimSet` per-tick in poll_loop — Codex が claim した pane_id は Step 6b から除外
- 問題: ClaimSet は CWD→pane_id binding であり、同一 CWD に両 agent がいる場合の source 間競合を解決しない。Gemini 等の追加時に poll_loop 修正が必要

**Option 2 (My — projection 2-pass)**: `is_bootstrap: bool` flag + projection 2-pass で bootstrap と heartbeat を分離
- 問題: projection と Step 6b の間でフラグを伝達する必要あり。ordering dependency が生まれ blast radius 大

**Option 3 (Codex reviewer — chosen)**: `cwd_candidate_count: usize` を source layer の `SessionDiscovery` に持たせる
- CWD に対して pane が 1 つなら → `bootstrap_event(is_heartbeat=false)` (従来通り)
- CWD に対して pane が 2+ なら → `ambiguous_cwd_bootstrap(is_heartbeat=true)` → `last_real_activity[Claude]` を書かない → `select_winning_provider` でそのままでは Claude が勝てない → Codex の `last_real_activity` が優先
- 最小 blast radius (source layer のみ変更)。poll_loop / projection / gateway の変更なし。Gemini/Copilot 等が将来追加されても自動的に恩恵を受ける

#### Fixes chosen
- **Bug A**: `cwd_candidate_count` in `SessionDiscovery` + `ambiguous_cwd_bootstrap(is_heartbeat=true)` in source.rs
- **Bug B**: `CLAUDE_RUNTIME_CMDS = ["node", "bun", "deno", "python", "python3"]` positive allowlist in Step 6b (poll_loop.rs) — `current_cmd` が allowlist にない pane は候補から除外
- **Bug C**: `detect()` early return — `process_hint=Some("shell") → return None` (detect.rs)

#### Files to change
- `crates/agtmux-source-claude-jsonl/src/discovery.rs`: `cwd_candidate_count` フィールド追加
- `crates/agtmux-source-claude-jsonl/src/source.rs`: `ambiguous_cwd_bootstrap()` + poll_files() 分岐
- `crates/agtmux-runtime/src/poll_loop.rs`: `CLAUDE_RUNTIME_CMDS` allowlist
- `crates/agtmux-source-poller/src/detect.rs`: shell early return (failing tests already written as spec)

### T-127: Pane attribution false-positive fixes — Completed

#### Bug C fix: detect.rs shell early return
- `detect(meta, def)` の先頭に `if meta.process_hint.as_deref() == Some("shell") { return None; }` を追加
- zsh/bash 等の shell pane が capture buffer に stale な agent 出力を持っていても heuristic 検出されなくなった
- 2 failing tests (spec) が PASS に: `detect_shell_pane_never_assigned_even_with_claude_output`, `detect_shell_pane_never_assigned_codex`

#### Bug B fix: poll_loop.rs CLAUDE_JSONL_RUNTIME_CMDS allowlist
- Step 6b filter を positive allowlist 方式に変更
- `CLAUDE_JSONL_RUNTIME_CMDS = ["node", "bun", "deno", "python", "python3"]` を定義
- `process_hint=None` の pane は `current_cmd` が allowlist に含まれる場合のみ JSONL discovery 候補に
- `process_hint=Some("claude")` → 常に含む、`Some("codex")|Some("shell")` → 除外、`Some(unknown)` → fail-closed で除外

#### Bug A fix: discovery.rs + source.rs cwd_candidate_count
- `SessionDiscovery.cwd_candidate_count: usize` を追加。同一 canonical CWD の候補 pane 数を事前集計
- `discover_sessions_in_projects_dir`: canonical CWD を事前解決し `HashMap<&str, usize>` でカウント、各 `SessionDiscovery` に埋め込む
- `source.rs poll_files()`: 初回 idle poll 時:
  - `count == 1` → `bootstrap_event(is_heartbeat=false)` (従来通り)
  - `count > 1` → `ambiguous_cwd_bootstrap(is_heartbeat=true)` — `last_real_activity` を書かない
- `ambiguous_cwd_bootstrap()` 関数追加 (`idle_heartbeat` と同じ内容だが用途/コメントが明確に区別)

#### Tests (4 new)
1. `detect_shell_pane_never_assigned_even_with_claude_output` — shell pane は Claude capture があっても None (detect.rs)
2. `detect_shell_pane_never_assigned_codex` — shell pane は Codex にも None (detect.rs)
3. `discover_sessions_cwd_candidate_count_multi_pane` — 2 pane 同一 CWD → count=2、単独 CWD → count=1 (discovery.rs)
4. `poll_files_emits_ambiguous_bootstrap_when_cwd_has_multiple_panes` — count=2 → is_heartbeat=true (source.rs)

#### Gate evidence
- 656 tests total (652 → 654 → 656), `just verify` PASS (fmt + lint + test)

#### Files changed
- `crates/agtmux-source-poller/src/detect.rs`: shell early return
- `crates/agtmux-source-claude-jsonl/src/discovery.rs`: `cwd_candidate_count`, `HashMap` import, refactor
- `crates/agtmux-source-claude-jsonl/src/source.rs`: `ambiguous_cwd_bootstrap()`, poll_files() branch
- `crates/agtmux-runtime/src/poll_loop.rs`: `CLAUDE_JSONL_RUNTIME_CMDS` allowlist + `snapshot_cmd` lookup

---

### T-128: Process-tree agent identification — Design Decision

#### Remaining problem after T-127

Live `agtmux list-panes` after T-127 fix showed:
- `%35` (Codex/node, CWD=test-session) → `codex deterministic` ✓ (fixed by T-126/T-127)
- `%297` (Claude Code/node, CWD=test-session) → `codex deterministic` ✗ (still wrong)

**Root cause chain**:
1. `%35` と `%297` は同一 CWD (`test-session`) を共有
2. `inspect_pane_processes("node")` → `None` (neutral) — `node` は Codex も Claude Code も同一コマンドで起動するため区別不能
3. T-124 (`build_cwd_pane_groups`) が両 pane を同一 CWD グループに入れ、Codex スレッドを `%35` + `%297` の両方に割り当てる
4. T-127 `ambiguous_cwd_bootstrap(is_heartbeat=true)` により `last_real_activity[Claude]` が書かれない
5. `select_winning_provider`: Claude に `last_real_activity` がない → Codex が unchallenged で勝つ
6. 結果: `%297` が `codex deterministic` になる

**本質**: `current_cmd` だけでは `node` の正体（Codex vs Claude Code）を判別できない。プロセスツリーの子プロセス argv を検査する必要がある。

#### Architectural approaches compared

**Claude agent 提案**:
- **B: JSONL 専有証明** — `~/.claude/projects/<cwd>/<session>.jsonl` の最新行 timestamp と pane_pid のプロセス起動時刻を照合。pane_pid の node プロセスが JSONL ファイルを書いていた証明
  - 問題: ファイル書き込みプロセスの追跡は macOS では `/proc` が無く `lsof` 依存。tick 毎の `lsof` は重すぎる
- **C: jsonl_path based** — JSONL discovery で pane に紐づく JSONL が見つかれば `process_hint=Some("claude")` に昇格
  - 問題: discovery は CWD ベースのため、同一 CWD の Codex pane も誤って claude に昇格しうる

**Codex reviewer 提案**:
- **P0: PaneBindingQuality core type** — binding に quality score (exact/inferred/fallback) を付与し UI で可視化
  - 問題: 根本的な誤帰属を解決しない。可視性の改善のみ
- **P1: process tree via pane_pid** ← **選択**
  - `#{pane_pid}` を tmux フォーマットで取得 → `TmuxPaneInfo.pane_pid: Option<u32>`
  - tick 先頭で `ps -eo pid=,ppid=,args=` を 1 回実行 → `ProcessMap` 構築
  - `inspect_pane_processes_deep(pane_pid, process_map)` で直接子プロセスの argv を検査
  - argv に `codex` → `Some("codex")`、`claude` (claude_desktop 除外) → `Some("claude")`、判別不能 → `Some("runtime_unknown")`
  - `runtime_unknown` = fail-closed: tier=2 (Codex 割り当て除外) + Step 6b 除外 → unmanaged
- **P2: CWD claim solver** — ILP 的な最適割り当て
  - 問題: 過剰複雑。P1 の直接証拠があれば不要

#### Decision: Codex P1

**理由**:
1. **直接証拠**: argv は process が Codex か Claude Code かを直接証明する — 推論ではなく事実
2. **将来性**: Gemini CLI や他 agent も `process_hint` で自動分類。struct 変更不要
3. **fail-closed**: `runtime_unknown` により誤帰属ではなく `unmanaged` に。偽陽性より偽陰性を選ぶ
4. **コスト**: `ps -eo pid=,ppid=,args=` は tick 1 回 — `lsof` のような per-file コストなし

#### Implementation plan (6 phases)

- **Phase 1**: `pane_pid: Option<u32>` を `TmuxPaneInfo` + `LIST_PANES_FORMAT` (#{pane_pid}) に追加
- **Phase 2**: `scan_all_processes() → ProcessMap` + `inspect_pane_processes_deep(pane_pid, map)` を `capture.rs` に実装
  - ProcessMap: `HashMap<u32, ProcessInfo { pid, ppid, args }>` (`ps -eo pid=,ppid=,args=`)
  - 子プロセス検索: ppid == pane_pid を全探索。argv に `codex`/`claude` を含むかチェック
- **Phase 3**: `to_pane_snapshot` が `pane_pid` + `ProcessMap` を受け取り deep inspection を呼ぶ
  - `pane_pid.is_none()` → `inspect_pane_processes(current_cmd)` (従来フォールバック)
  - `process_hint` の出力: `Some("codex")` / `Some("claude")` / `Some("runtime_unknown")` / `None`
- **Phase 4**: `pane_tier()` に `runtime_unknown` → tier=2 を追加 (Codex 割り当て除外)
- **Phase 5**: Step 6b フィルタ — `Some("runtime_unknown")` を `Some("codex")|Some("shell")` と同様に除外
  - T-127 ambiguous 条件 (`cwd_candidate_count > 1`) も `process_hint=Some("claude")` で精緻化可能
- **Phase 6**: Tests (unit + live) + `just verify`

#### Expected outcome
- `%297` (Claude Code/node): `inspect_pane_processes_deep` → argv に `claude` → `process_hint=Some("claude")`
- `%35` (Codex/node): argv に `codex` → `process_hint=Some("codex")`
- 両 pane が正確に識別され、`%297` が `claude deterministic` に

### T-128: Process-tree agent identification — Completed

#### Implementation (6 phases)

**Phase 1: `pane_info.rs`**
- `LIST_PANES_FORMAT` に `\t#{pane_pid}` を追加 (13番目フィールド)
- `TmuxPaneInfo.pane_pid: Option<u32>` を追加
- `parse_line`: 13番目フィールドを optional でパース (`parse::<u32>().ok()`)
- 後方互換: 12 フィールド時は `pane_pid = None`

**Phase 2: `capture.rs`**
- `ProcessInfo { pid, ppid, args }` struct を追加
- `ProcessMap = HashMap<u32, ProcessInfo>` type alias を追加
- `scan_all_processes() -> ProcessMap`: `ps -eo pid=,ppid=,args=` を 1 回実行
- `parse_ps_output(output)` / `parse_ps_line(line)` private helpers
- `inspect_pane_processes_deep(current_cmd, pane_pid, process_map)`:
  - Fast path: `shell`/`codex`/`claude` は shallow inspection で即返す
  - `pane_pid` 自身 + 直接子プロセスの argv を `is_claude_argv` / `is_codex_argv` でチェック
  - 子プロセスあり・両方 miss → `Some("runtime_unknown")` (fail-closed)
  - 子なし → shallow fallback (`None` for neutral runtime)
- `is_claude_argv`: `"claude"` 含む && `"claude_desktop"` / `"claude-desktop"` 除外
- `is_codex_argv`: `"codex"` 含む

**Phase 3: `snapshot.rs`**
- `to_pane_snapshot` に `process_map: Option<&ProcessMap>` 引数を追加
- `(pane.pane_pid, process_map)` が両方 `Some` の場合 `inspect_pane_processes_deep` を呼ぶ、そうでなければ shallow

**Phase 4: `codex_poller.rs`**
- `pane_tier()`: `Some("runtime_unknown") => 3` を `shell` と同じ tier=3 に (明示 arm 追加)
- 結果: `runtime_unknown` pane は unclaimed プールに入らず、Codex thread を受け取らない

**Phase 5: `poll_loop.rs`**
- import: `scan_all_processes` を追加
- Step 2.5: `tokio::task::spawn_blocking(scan_all_processes).await.unwrap_or_default()` で `ProcessMap` 構築
- `to_pane_snapshot` に `Some(&process_map)` を渡す

**`lib.rs`**
- `ProcessInfo`, `ProcessMap`, `inspect_pane_processes_deep`, `scan_all_processes` を pub re-export

#### Tests (19 new)

- `pane_info`: `parse_with_pane_pid`, `parse_without_pane_pid_defaults_to_none`, `parse_pane_pid_invalid_value_defaults_to_none` (3)
- `capture` (parse_ps / deep inspect): `parse_ps_output_basic`, `parse_ps_output_empty_lines_skipped`, `parse_ps_output_no_args`, `deep_inspect_claude_child`, `deep_inspect_codex_child`, `deep_inspect_runtime_unknown_when_child_unidentifiable`, `deep_inspect_no_children_falls_back_to_shallow`, `deep_inspect_shell_fast_path`, `deep_inspect_explicit_codex_cmd_fast_path`, `deep_inspect_excludes_claude_desktop` (10)
- `snapshot`: `snapshot_deep_inspection_claude_child`, `snapshot_deep_inspection_codex_child`, `snapshot_deep_inspection_runtime_unknown`, `snapshot_deep_inspection_no_children_falls_back` (4)
- `codex_poller`: `pane_tier_runtime_unknown_is_tier3`, `process_thread_list_runtime_unknown_panes_never_assigned` (2)

#### Gate evidence
- 675 tests total (656 → +19), `just verify` PASS (fmt + lint + test)

#### Expected live fix
- `%35` (Codex/node): `inspect_pane_processes_deep` → child argv contains "codex" → `process_hint=Some("codex")` → tier=0 → Codex thread ✓
- `%297` (Claude Code/node): child argv contains "claude" → `process_hint=Some("claude")` → tier=2 → deprioritized for Codex
  - CWD 候補が `%297` のみ (codex pane 除外後) → `cwd_candidate_count=1` → `bootstrap_event(is_heartbeat=false)` → `last_real_activity[Claude]` 設定 → Claude wins ✓

---

### T-126: JSONL all-pane discovery fix — Completed (3 phases)

#### Root cause (confirmed)
`poll_loop.rs` Step 6b gated JSONL discovery on `claude_pane_ids` (panes poller/projection already knew were Claude).
After daemon restart, projection is empty → `claude_pane_ids` = {} → `discover_sessions` never called → no heartbeat → Codex CWD assignment wins for idle Claude panes.
**Vicious cycle**: JSONL discovery gated on Claude detection; Claude detection requires JSONL evidence after restart.

#### Phase 1: Remove `claude_pane_ids` filter + process_hint exclusion
- Removed `claude_pane_ids` filter from Step 6b; replaced with `snapshot_hint` lookup from `snapshots` vector
- Filter: exclude `Some("shell") | Some("codex")` panes (prevents attributing Claude to zsh panes that happen to share CWD with old JSONL files)
- `candidate_pane_cwds` = all panes except definite non-Claude processes
- `discover_sessions` returns empty for panes with no `~/.claude/projects/<cwd>/*.jsonl` → safe (no false positives for panes with no JSONL)

#### Phase 2: Bootstrap event in watcher
**Problem after Phase 1**: idle watcher emitted `is_heartbeat=true` even on first poll. This only refreshes `deterministic_last_seen`, NOT `last_real_activity[Claude]`. So `select_winning_provider` couldn't see Claude as "recently active" — Codex still won.

**Fix:**
- Added `bootstrapped: bool` field to `SessionFileWatcher` (starts `false`)
- `is_bootstrapped()` / `mark_bootstrapped()` accessors
- `poll_files()` first-poll logic: if no real events → emit `bootstrap_event(is_heartbeat=false)`, then set `bootstrapped=true`
- `bootstrap_event()`: `is_heartbeat=false`, `event_type="activity.idle"` — writes `last_real_activity[Claude]` in projection
- If real events were emitted on first poll: mark bootstrapped, no extra bootstrap event needed
- Second+ polls: emit `idle_heartbeat(is_heartbeat=true)` as before

#### Phase 3: Timing fix — `Utc::now()` in Step 6b
**Problem after Phase 2**: Step 6b used poll_tick's `now` (set at tick START). Step 6a (Codex network I/O) uses `Utc::now()` internally during async call. So T_codex > T_tick_start → `last_real_activity[Codex] > last_real_activity[Claude]` → Codex won `select_winning_provider`.

**Fix:** Changed `poll_files(..., now)` → `poll_files(..., Utc::now())` in Step 6b.
- Step 6b runs AFTER Step 6a completes → T_claude = Utc::now() ≥ T_codex → Claude wins provider conflict for idle Claude panes

#### Tests
- `poll_tick_jsonl_discovery_scans_all_panes` (new): verifies node pane with no JSONL gets discovery attempted without panic
- `poll_files_emits_bootstrap_on_first_poll_when_no_new_lines` (renamed + updated): first poll = bootstrap (is_heartbeat=false), second poll = heartbeat (is_heartbeat=true)
- `poll_files_emits_bootstrap_when_only_metadata_lines` (renamed + updated): metadata-only first tick = bootstrap, not heartbeat
- 652 tests total (from 649), `just verify` PASS

#### Live verification
- `%297` (test-session, node, CWD=agtmux-daemon): `claude deterministic idle` ✓ (was `codex deterministic`)
- `%290` (exp-go-codex, node): `claude deterministic idle` ✓
- `%282`, `%289` (real Codex panes): `codex deterministic idle` ✓ (unaffected)
- All zsh panes: `unmanaged` ✓ (no false Claude attribution)

#### Files changed
- `crates/agtmux-runtime/src/poll_loop.rs`: Step 6b completely rewritten (snapshot_hint, candidate_pane_cwds, Utc::now())
- `crates/agtmux-source-claude-jsonl/src/watcher.rs`: `bootstrapped` field + accessors
- `crates/agtmux-source-claude-jsonl/src/source.rs`: `bootstrap_event()`, `poll_files()` bootstrap logic, 2 test renames

---

## 2026-02-27
### T-125: Shell pane false-positive Codex binding fix — Completed

### Problem confirmed via live inspection
- `inspect_pane_processes("zsh")` → `None` = neutral tier 1 (同 `node`)
- App Server が CWD 共有 pane 全体にスレッドを割り当て → zsh pane が `codex deterministic` に
- 実例: vm agtmux v4 の %286, %305 (zsh) が誤って managed、test-session %301 (zsh) も同様

### Implementation
- `SHELL_CMDS` 定数: zsh, bash, fish, sh, csh, tcsh, ksh, dash, nu, pwsh
- `inspect_pane_processes`: SHELL_CMDS に完全一致 → `Some("shell")` 返却 (exact match, lowercase)
- `pane_tier()`: `Some("shell")` → tier 3 (never assign)
- `process_thread_list_response` unclaimed フィルタ: `pane_tier(p) < 3` 追加
- 4 new tests: `inspect_shell_cmds`, `inspect_neutral_runtime`, `build_cwd_pane_groups_tier_sort_with_shell`, `process_thread_list_shell_panes_never_assigned`
- 649 tests total, `just verify` PASS
- live 確認: v4 zsh pane, test-session %301 が unmanaged に ✓

### Claude JSONL 検出失敗調査 (→ T-126)

#### 問題: test-session %297 が `codex deterministic` になっているが、実際は Claude idle

#### 調査結果
- %297 CWD: `/Users/virtualmachine/ghq/github.com/yohey-w/multi-agent-shogun/agtmux-rs/crates/agtmux-daemon`
- JSONL ファイル: `~/.claude/projects/-Users-...-agtmux-daemon/76b99a53-9c1a-4800-8916-71e31dddc920.jsonl` (2217行) → **存在する**
- JSONL 最終書き込み: 2026-02-26 17:48 JST
- daemon 起動時刻: 2026-02-26 20:48 JST (3時間後)
- 最終行の type: `system` → translate が `None` を返す (無視)
- watcher 設計: **EOF 起点** = 起動時に全履歴をスキップ
- Claude は3時間 idle → 新規 JSONL 行なし → watcher イベントなし
- 結果: Claude JSONL 証跡なし → Codex CWD 割り当てが勝つ

#### 根本原因
**watcher の EOF 起点設計が「daemon restart 後の idle Claude pane」を検出できない**。
Codex は App Server が常に現在スレッドリストを返すため問題なし。
Claude は JSONL への新規書き込みが発生するまで证跡が得られない。

#### 提案 Fix (T-126): last-line bootstrap
- watcher 起動時に EOF から逆方向スキャン
- 最後の meaningful line (assistant / user / progress type) を1行だけ emit
- 以降は通常の EOF watch に切り替え
- ⚠️ 注意: last line が `system` 等の skip 対象の場合は emit しない

---

## 2026-02-26 (cont.)
### T-124: Same-CWD Multi-Pane Codex Binding — Planning

### Problem
- `build_cwd_pane_map`: `HashMap<CWD, PaneCwdInfo>` — CWD ごと 1 pane のみ保持。同一 CWD 複数 pane が unmanaged に。
- ライブテスト: vm agtmux v4 の 4 pane が全て `/agtmux=v4` CWD → 1 pane のみ managed
- Fix 1 (適用済み commit db024a9): `MAX_CWD_QUERIES_PER_TICK` 8 → 40

### Design decisions
- `build_cwd_pane_groups`: `HashMap<CWD, Vec<PaneCwdInfo>>` — 全 pane をグループ化
- pane ソート: `has_codex_hint desc, pane_id asc` — 実際の Codex pane を優先割り当て
- thread ソート: `thread_id asc` — 安定割り当て
- stable assignment: cache-first + VecDeque unclaimed
- **H1 (Codex review)**: cache hit に `pane_id + generation + birth_ts` 一致チェック追加。pane 再利用時に古い binding を invalidate
- **H2 (Codex review)**: tick-scope `assigned_in_tick: HashSet<String>` — 同一 thread が複数 CWD クエリに出現しても先着固定（cwd filter 異常対策）
- **H3 (Codex review)**: pane ソートを `has_codex_hint desc, pane_id asc` に変更（元案は `pane_id asc` のみ）
- Global query (`cwd=None`) は `&[]` 渡し → 新規割り当てなし、既存 binding の heartbeat のみ

### `has_codex_hint: bool` → `process_hint: Option<String>` 置き換え決定
- `has_codex_hint` は旧 1-pick-per-CWD アルゴリズム向けの情報損失 shortcut
- T-124 の多 pane 割り当てでは 3-tier sort が必要: codex(0) > neutral(1) > competing-agent(2)
- `process_hint: Option<String>` を `PaneCwdInfo` に直接保持する設計に変更
- Gemini 等が将来 `inspect_pane_processes()` に追加されても struct 変更なしで tier 2 に自動分類

### Codex review 結果
- 判定: **Go with changes** (確信度: Medium)
- 3 High リスク → 全採用 (H1/H2/H3)
- Medium リスク (stale binding TTL, event_id collision) → 将来 hardening で対応

---

## 2026-02-26 (cont.)
### T-124: Same-CWD Multi-Pane Codex Binding — Completed

### Implementation
- `build_cwd_pane_map` (HashMap<CWD, PaneCwdInfo>) → `build_cwd_pane_groups` (HashMap<CWD, Vec<PaneCwdInfo>>): keeps ALL panes per CWD, sorted by 3-tier pane_tier() + pane_id
- `has_codex_hint: bool` → `process_hint: Option<String>` in `PaneCwdInfo` (codex_poller.rs + poll_loop.rs); `pane_tier()` free function: codex=0, neutral(None)=1, competing-agent=2
- `process_thread_list_response` new signature: `pane_infos: &[PaneCwdInfo]`, `assigned_in_tick: &mut HashSet<String>`
- H1: `cached_pane_ids` only marks generation-valid bindings as "claimed" → stale bindings release their pane into unclaimed
- H2: `assigned_in_tick` guards against same thread reassignment across CWD queries in same tick
- Global query (`cwd=None`) → `&[]` → no new assignments, only heartbeat continuity
- Bug fix discovered in test: `cached_pane_ids` must exclude generation-invalid bindings, not just absent bindings
- 14 new tests: 4 `build_cwd_pane_groups_*` + 6 `process_thread_list_*` (4 as `#[tokio::test]` using `make_test_client()` backed by `cat` subprocess)
- Also fixed: 7 `.unwrap()` → `.expect()` in projection.rs test code (clippy::unwrap_used)
- 645 tests total (up from 631), `just verify` PASS

### Key decisions
- `process_hint: Option<String>` propagated directly from `PaneSnapshot` → `PaneCwdInfo` (no bool lossy conversion)
- Gemini/Copilot future agents auto-classify to tier 2 without struct changes
- `make_test_client()` uses `cat` subprocess (tokio::process) to satisfy opaque tokio Child/ChildStdin/ChildStdout types without actual Codex binary dependency

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

---

## 2026-02-27 — Phase 6 CLI/TUI: T-130 / T-131 / T-132 Completed

### T-130: build_pane_list フィールド追加
- `session_id` ($N)、`window_id` (@N)、`current_path` を managed/unmanaged 両方の JSON レスポンスに追加
- 変更: `crates/agtmux-runtime/src/server.rs` の `build_pane_list()` の managed/unmanaged 両ブロック
- 2 new tests (managed + unmanaged パス確認). 681 tests total. `just verify` PASS.

### T-131: agtmux list-windows コマンド
- `cli.rs`: `ListWindows(ListWindowsOpts)` variant + `--color=always/never/auto`
- `client.rs`:
  - `format_windows(panes, use_color) -> String` — unit-testable な純関数
  - 階層: `session (N windows — X Running, Y Idle)` → `@N name — stats` → pane lines
  - managed: `* provider [det/heur] State  current_path`
  - unmanaged: `— cmd  [unmanaged]`
  - window sort: `@` prefix を除去して数値ソート (lexicographic 問題を解決)
  - color auto: `std::io::IsTerminal` で TTY 判定
  - `cmd_list_windows()` — RPC call → format → println
- `main.rs`: `ListWindows(opts)` → `cmd_list_windows(&socket, &opts.color).await?`
- 7 new tests. 688 tests total. `just verify` PASS.

### T-132: fzf レシピ + README
- `README.md` 新規作成:
  - Quick Start (daemon / setup-hooks / list-windows)
  - `agtmux list-windows` 出力フォーマット例
  - fzf ワンライナー: `agtmux list-windows --color=always | fzf --ansi | grep -oE '@[0-9]+' | xargs tmux select-window -t`
  - `.tmux.conf` スニペット (bind-key C-w)、`alias aw` の shell alias
  - `tmux status-right` スニペット (`agtmux tmux-status`)
  - コマンド一覧テーブル

### Gate evidence
688 tests total (679 → 681 → 688), `just verify` PASS (fmt + lint + test)

---

## 2026-02-27 — Phase 6 Wave 2 設計決定: CLI 表示リデザイン方針

### 背景
T-131 (list-windows) の初版実装後にユーザーから UI 設計フィードバックを受けた。GUI のサイドバー（スクリーンショット参照）と照合し、CLI の表示設計を根本から見直した。

### 主な設計判断

| 判断 | 採用 | 理由 |
|------|------|------|
| @N/@M (window/pane ID) 非表示 | ✅ | users は window_name で考える。@N はシステム内部の識別子。fzf は `session:window_name` で動作可能 |
| det = 無印、heur = `~` prefix | ✅ | det が「期待される通常状態」。heur だけが例外を示す。`[det]`/`[heur]` の両表示は冗長 |
| path はデフォルト非表示 (`--path`) | ✅ | agent title が分かれば十分。path は optional 情報 |
| `list-panes` のデフォルト出力を JSON → human-readable に変更 | ✅ | `--json` で後方互換。daily use での可読性を優先 |
| conversation title は後続タスク (T-135) | ✅ | 最大の価値だが取得経路未実装。T-133/T-134 で表示層を先に確定し、T-135 で data を差し込む |

### conversation title の現状ギャップ
GUI が示す最大の価値 (「think 10s」「_AGTMUX V3 Redesign」) は会話タイトル。現在の `title` フィールドは provider 名か UUID fallback。Claude JSONL の `sessions-index.json` や JSONL `summary` フィールドからの抽出が必要 (T-135)。

### 3コマンド構造（確定）
- `list-panes`: フラット・ペイン単位。sidebar 相当。pane 切り替え用 fzf。
- `list-windows`: window 単位集計。@N 非表示、window_name のみ。window 切り替え用 fzf。
- `list-sessions`: session 単位集計。session 切り替え用 fzf。

### 実装順序
T-133 (`list-panes` リデザイン) + T-134 (`list-windows` リデザイン + `list-sessions` 新規) → T-135 (title 抽出)
- T-133 と T-134 は独立。並行実施可能。
- T-135 は T-133/T-134 完了後に着手（表示層確定後に data layer 追加）。

---

## 2026-02-27 — T-133/T-134 CLI display redesign — Completed

### T-133: list-panes redesign
- `format_panes(panes, show_path, use_color)`: session-grouped sidebar (first-seen order), panes sorted numerically
- det managed panes: `    {title:<30}  {rel}` (no marker)
- heur managed panes: `  ~ {title:<30}  {rel}` (yellow `~` in color mode)
- unmanaged panes: `    {cmd}` (dim in color mode)
- @N/@M/%N IDs completely hidden from output
- `--json`: JSON raw output (backward compat)
- `--path`/`-p`: append `current_path` suffix
- `--color=always/never/auto`
- Helpers: `relative_time()`, `resolve_color()`, `provider_short()` (ClaudeCode→Claude)

### T-134: list-windows redesign + list-sessions new
- `format_windows(panes, show_path, use_color)`: @N IDs hidden → window_name only, %N IDs hidden, `[det]`/`[heur]` tags removed → unified `~` prefix for heur, show_path support, relative_time per pane
- `format_sessions(panes, use_color)`: one line per session: `{name}  {N} window(s)  {M} agent(s) (Running/Idle/Waiting)  {K} unmanaged`
- `cmd_list_sessions(socket_path, color)` added (was missing)
- cli.rs: `ListSessions(ListSessionsOpts)` + `ListPanes(ListPanesOpts)` added; `ListWindowsOpts` got `--path`/`-p`

### Tests
- 17 new tests: 8 format_panes + 4 format_windows (updated/new) + 5 format_sessions
- 690 → 707 total tests
- `just verify` PASS (fmt + clippy + test)

### Files changed
- `crates/agtmux-runtime/src/client.rs`
- `crates/agtmux-runtime/src/cli.rs`
- `crates/agtmux-runtime/src/main.rs`

## 2026-02-27 — T-133/T-134 post-review fixes

### Trigger
Dual review (Claude + Codex) on T-133/T-134 changes. Both identified issues resolved.

### Claude reviewer findings (Go with changes)
- Missing tests: `conversation_title` priority, null fallback, missing `updated_at`
- README: `list-panes | jq .` broken without `--json`; fzf recipes use `@N` IDs (now hidden)

### Codex reviewer findings (P2)
- `README.md:47`: `agtmux list-panes | jq .` now broken → needs `--json` flag
- `README.md` fzf section: `grep -oE '@[0-9]+'` no longer matches hidden IDs

### Fixes applied
- `README.md` fully rewritten: fixed `--json` flag, new `list-panes`/`list-sessions` sections, fzf recipes use awk-based `session:window_name` extraction (no @N dependency)
- 4 new tests added to client.rs:
  - `format_panes_conversation_title_overrides_provider`
  - `format_panes_conversation_title_null_falls_back_to_provider`
  - `format_panes_updated_at_missing_shows_no_time`
  - `format_windows_empty_window_name_shows_unnamed`
- 707 → 711 total tests
- `just verify` PASS

---

## 2026-02-27 — T-136 Waiting 表示バグ修正 + E2E 計画策定

### T-136 完了 (711 → 713 tests)
`ActivityState::WaitingInput` / `WaitingApproval` が `format!("{:?}", ...)` で Debug 文字列 (例: `"WaitingInput"`) として JSON 出力されるが、client.rs の 5 箇所で `"Waiting"` リテラルと照合していたため、永遠にカウント 0 になるバグ。

**修正箇所 (client.rs)**:
1. `format_windows` `sess_waiting` 集計: `Some("Waiting")` → `Some("WaitingInput") | Some("WaitingApproval")`
2. `format_windows` `win_waiting` フィルター: `.filter()` 内を `matches!()` マクロに変更
3. `format_windows` pane 表示: `display_state` 変数で正規化 (`WaitingInput/WaitingApproval → "Waiting"`)、no-color non-heur ブランチも `{display_state}` に修正
4. `format_sessions` `waiting` 集計: 同様
5. 追加テスト: `format_windows_waiting_input_normalized` / `format_sessions_waiting_approval_counted`

### E2E 計画 (T-137/T-138) 策定
- 3-layer アーキテクチャ: Unit(711) / Contract E2E / Detection E2E
- Contract E2E: `source.ingest` RPC で合成イベント注入 (実 CLI 不要)
- Detection E2E: provider-adapter パターン (Gemini 等の追加も adapter のみ)
- 詳細: `.claude/plans/gleaming-prancing-wilkes.md`

---

## 2026-02-28 — Phase 6 Wave 3 設計決定: Context-aware CLI 表示

### Trigger
CLI の情報量設計について、以下 2 パターンの長所を統合する方針を確定した。
- Codex app: main panel に `cwd/git` など文脈、sidebar は session title 中心（高い scanability）
- cmux: sidebar に title + summary + cwd（高い情報密度）

### Decision
- **原則**: 「default は軽く、文脈は header に集約し、差分のみ pane 行へ出す」
- `list-panes`:
  - default は `title + state + relative_time`（unmanaged は `current_cmd`）
  - `--context=auto|off|full` を導入
  - `auto`（default）: `cwd/git` を session/window header に表示し、pane ごとの差分のみ suffix 表示
- `list-windows` / `list-sessions`:
  - context は集約表示を基本とする
  - 同一 window/session 内で `cwd/git` が混在する場合は `mixed` marker を表示
- summary:
  - `--summary` opt-in（default off）
  - deterministic source（AppServer/hooks/JSONL）由来のみ表示
  - capture/title 由来の推測 summary は表示しない

### Why
- daily use では pane 一覧の視認速度が最重要。context を pane 行へ常時表示するとノイズが増える。
- 一方で CWD/branch の文脈は切替判断に有効。header 集約 + 差分表示で情報密度と可読性を両立できる。
- summary を default 表示すると誤推測や stale 情報が混入しやすいため、opt-in + deterministic 限定で fail-closed にする。

### Follow-up tasks
- T-139: `--context=auto|off|full` 導入
- T-140: window/session context 集約 + `mixed` marker
- T-141: `--summary` opt-in（deterministic only）

### Cross-review feedback triage (Claude x2)

Adopted:
- `auto` 差分比較基準を明文化: 直近 window header、fallback で session header
- 差分判定条件を明文化: `cwd` または `git branch` が異なれば suffix 表示
- `list-windows` / `list-sessions` の default を `--context=auto` で統一
- `mixed` 表示に導線を追加: `mixed (use --context=full for detail)`
- context 同一性判定を fail-closed 化（欠損混在も `mixed`）
- `--path`/`-p` を `--context=full` alias として維持（互換）
- summary 文言を user-facing に修正（agent 明示データのみ）
- `--summary` で全欠損時の表示 `(no agent summaries available)` を追加
- `T-141 blocked_by` を `T-135b` から `T-139` へ変更（title 抽出依存を解消）

Not adopted:
- header 行に `#` / `##` プレフィックスを必須化する提案
  - 理由: 既存の可読性・fzf レシピ互換を維持するため、header/pane の機械判別はインデント契約（0/2/4 spaces）で固定する方針を採用

### Round 2 follow-up (latest parallel review反映)

追加で採用:
- `full` の command別仕様を明文化:
  - `list-panes`: pane 行に `cwd/branch` 常時表示
  - `list-windows`: 1行/window を維持しつつ `cwd/branch` 常時表示
  - `list-sessions`: 1行/session を維持しつつ `cwd/branch` 常時表示
- `auto` の OR/AND 混同を回避するため、`cwd` / `branch` をフィールド単位で独立判定に統一
- `mixed` 判定をフィールド単位 fail-closed として再明確化（不一致/欠損混在）
- summary の表示位置を固定（pane 行直下）。全欠損時メッセージは出力末尾 1 回のみ
- 例示を spec に合わせて修正（session レベルでも `mixed` が可視化されるケース）
- UX 出力契約の golden fixture タスクを追加（T-142、5ケース固定）

### Round 3 follow-up (parallel review反映)

追加で採用:
- `off` モードの出力契約を明文化（`cwd/branch` + `mixed` marker を非表示）
- `list-windows` / `list-sessions` の `auto` は window/session 行の集約 context を常時表示（親との差分抑制なし）
- `mixed` 表示を `[field=mixed]` へ統一し、`(use --context=full for detail)` は行末 1 回のみ表示（重複抑制）
- `full` の集約コマンド仕様を再明確化（1行/window, 1行/session 維持 + 集約値表示）
- pane 側欠損値表記を `<unknown>` に統一
- summary 欠損メッセージの表示位置を「全出力末尾 1 回のみ」に固定
- `--path` は互換 alias として維持しつつ、`-p` は deprecated として整理
- `design` 例をルールと整合する形へ更新（session/window の field-labeled `mixed`、pane差分表示）

### Round 4 follow-up (parallel review反映 + root policy update)

追加で採用:
- `--path` / `-p` を完全廃止。context 詳細化は `--context=full` のみ（後方互換なし方針）
- `list-panes --context=full` の header 挙動を固定（session/window header は `auto` 集約表示を維持）
- `single-window session` の window header は「省略可能」ではなく「常に省略」に固定
- `mixed` ガイダンスの適用単位を明文化（mixed 行ごとに 1 回、同一行で重複なし）
- summary 欠損 pane の挙動を固定（summary 行を出さない、placeholder なし）
- `deterministic-only` という内向き用語を、user-facing には「agent 明示の構造化 summary」の語に置換

### Round 5 follow-up (parallel review反映)

追加で採用:
- `auto` と `full` の違いを明確化（`auto` は差分/混在フィールドのみ pane suffix、`full` は全表示行で context 表示）
- mixed sentinel を `<mixed>` に統一し、欠損 `<unknown>` と同じ表記規約へ揃える
- mixed ガイダンスの重複抑制を強化（同一 session block の最上位 mixed 行のみ表示）
- `--summary` の all-missing 例を修正（pane 行を維持し、末尾 footer を追加）
- summary 行インデント規約を明文化（pane 行 +2 spaces）
- summary partial-missing ケースを golden fixture に追加（T-142: 5→6 ケース）

### Round 6 follow-up (parallel review反映)

追加で採用:
- FR-049a を新設し、single-window session の window header 省略を spec 本文へ昇格
- `list-panes auto` の header/pane の責務分離を FR-050 に追記（header 常時集約表示、差分ルールは pane のみ）
- `full` の「行数を増やさない」を「既存行への inline 追加」として再定義
- `--summary all-missing` 例を修正（pane 行を保持し、末尾 footer を追加）
- pane インデント規約を「親行 +4 spaces」に明文化

### Round 7 follow-up (parallel review反映)

追加で採用:
- mixed ガイダンス表示位置を deterministic に固定（session mixed 優先、なければ最初の mixed window 行）
- `--path` / `-p` 入力時の fail-closed エラー文言（`hint: use --context=full`）を仕様化
- `full` の「行数不変」説明を inline 追加の文言へ統一

### Round 8 follow-up (root policy update)

追加で採用:
- `-p` 方針を根本確定: list 系では未割り当てのまま固定し、別意味 short flag に再利用しない
- 表示密度制御の入口を `--context=...` の long option に一本化（メンタルモデルを 1 つに固定）
- T-139 に reject contract test（`-p` / `--path` の exit code + hint）を追加し、仕様逸脱を防止

---

## 2026-02-28 — T-136: Waiting 表示バグ修正 — Completed

### 問題
`server.rs:328` が `format!("{:?}", pane.activity_state)` で Debug 文字列 (`"WaitingInput"`, `"WaitingApproval"`) を出力していたが、`client.rs` は `"Waiting"` で照合していた。全 5 箇所で永久に 0 になるバグ。

### 修正内容
`crates/agtmux-runtime/src/client.rs`:
- `sess_waiting` 集計: `Some("Waiting")` → `Some("WaitingInput") | Some("WaitingApproval")`
- `win_waiting` filter: 同上
- pane 着色 (2 箇所): `"Waiting" => yellow` → `"WaitingInput" | "WaitingApproval" => yellow "Waiting"`
- `format_sessions` waiting 集計: 同上
- テスト 2 件追加: `format_panes_waiting_input_counted`, `format_sessions_waiting_approval_counted`

### Gate evidence
- 713 tests total, `just verify` PASS

---

## 2026-02-28 — T-137: Layer 2 Contract E2E 基盤 — Completed

### 新規ファイル
- `scripts/tests/e2e/harness/common.sh` — `wait_for_agtmux_state`, `assert_field`, `log`, `fail`, `pass`, `register_cleanup`
- `scripts/tests/e2e/harness/daemon.sh` — `daemon_start`, `daemon_stop` (UDS ready polling)
- `scripts/tests/e2e/harness/inject.sh` — `inject_claude_event`, `inject_codex_event`, event loop variants
- `scripts/tests/e2e/contract/test-schema.sh` — required JSON fields, types, ranges
- `scripts/tests/e2e/contract/test-claude-state.sh` — tool_start→Running, idle→Idle, wait_for_approval→Waiting
- `scripts/tests/e2e/contract/test-codex-state.sh` — thread.active→Running, thread.idle→Idle, recovery
- `scripts/tests/e2e/contract/test-waiting-states.sh` — WaitingInput/WaitingApproval → "Waiting" 表示
- `scripts/tests/e2e/contract/test-list-consistency.sh` — list-windows/list-sessions vs list-panes 整合性
- `scripts/tests/e2e/contract/test-multi-pane.sh` — 同一 session 複数 pane 独立管理
- `scripts/tests/e2e/contract/run-all.sh` — 全テスト実行 + 集計
- `justfile`: `preflight-contract`, `e2e-contract` targets 追加

### 主要バグ (発見・修正済み)
1. `jq -n` → `jq -nc`: pretty-print JSON は server `read_line` で最初の `{` 行しか読まれずパース失敗
2. `inject_*_event_loop` の `$()` ブロック: bash `$()` パイプの write-end を background subshell が継承 → `>/dev/null &` で修正
3. inject.sh の event_type 誤り: `task.running`/`task.idle` は `parse_activity_state()` に未定義 → `thread.active`/`thread.idle` に修正

### Gate evidence
- `just e2e-contract`: 6 passed, 0 failed

---

## 2026-02-28 — T-138: Layer 3 Provider-Adapter Detection E2E — Completed

### 新規ファイル

**Adapters** (provider-specific, 3 functions each: launch_provider / wait_until_provider_running / wait_until_provider_idle):
- `scripts/tests/e2e/providers/claude/adapter.sh` — claude --dangerously-skip-permissions -p; tmux capture pattern detection
- `scripts/tests/e2e/providers/codex/adapter.sh` — codex --full-auto; tmux capture pattern detection
- `scripts/tests/e2e/providers/gemini/adapter.sh.stub` — stub with implementation guide

**Scenarios** (provider-agnostic; sourced adapter is interchangeable):
- `scenarios/single-agent-lifecycle.sh` — Running → Idle lifecycle + evidence_mode=deterministic
- `scenarios/multi-agent-same-session.sh` — 2 agents same session, different CWD → both managed
- `scenarios/same-cwd-multi-pane.sh` — T-124 regression: 2 panes same CWD → both managed
- `scenarios/provider-switch.sh` — PROVIDER_A stops → PROVIDER_B starts in same pane (cross-provider arbitration)

**Orchestrator**:
- `online/run-all.sh` — PROVIDER= env var, E2E_SKIP_SCENARIOS support, auto-skip platform-specific tests

### 3層アーキテクチャ完成

| Layer | コマンド | 必要物 |
|-------|---------|--------|
| Layer 1: Unit | `just verify` (713 tests) | Rust のみ |
| Layer 2: Contract | `just e2e-contract` (6 tests) | tmux + python3 + jq |
| Layer 3: Detection | `just e2e-online-claude` / `just e2e-online-codex` | tmux + Claude/Codex CLI + auth |

### Gate evidence
- 全ファイル syntax check PASS (`bash -n` for all scripts)
- adapter path resolution test PASS
- live CLI test: requires `just preflight-online` (ANTHROPIC_API_KEY / OPENAI_API_KEY)

---

## 2026-02-28 — Post-review fixes (Reviewer A NB-4 + Reviewer B B-1)

### B-1 Critical: test-waiting-states.sh False Confidence — FIXED

**問題**: `test-waiting-states.sh` が `tool_start` (→ Running) のみ inject し、
`WaitingApproval`/`WaitingInput` 状態を一切生成していなかった。
`assert_not_contains "WaitingApproval"` が Trivially true (pane は Running のため)。

**根本原因**: `inject_claude_event` は `hook_type: "wait_for_approval"` を
`translate.rs` の `normalize_event_type()` を通じて `lifecycle.unknown` → `ActivityState::Unknown` にマップ。
`WaitingApproval` を生成する hook type が Claude hooks に存在しなかった。

**修正**: `inject_codex_event_loop` で `event_type: "lifecycle.waiting_approval"` / `"lifecycle.waiting_input"` を注入。
`CodexRawEvent.event_type` は plain `String` なので any event_type を受け付ける。
`parse_activity_state("lifecycle.waiting_approval")` → `ActivityState::WaitingApproval` が確定的に機能。

**結果**: Scenario 1 で WaitingApproval 到達を `wait_for_agtmux_state` で確認してから assertions を実行。
Scenario 2 で WaitingInput も同様に確認。6 つのアサーション (list-windows/list-sessions 表示 + JSON raw 保全) が全て実 False→True のチェックに。

### NB-4 Non-blocking: ANSI padding in format_windows — FIXED

**問題**: `{state_str:<7}` で ANSI escape code 込みの文字列を pad すると、
escape bytes を含む raw 長を基準に pad するため color mode では列が揃わない。
例: `"\x1b[32mRunning\x1b[0m"` (14 bytes) の :<7 は追加 padding なし → "Idle" との列ズレ。

**修正**: pad を color code 付与の前に実施:
```rust
let padded = format!("{display_state:<7}");
let state_str = match display_state {
    "Running" => format!("\x1b[32m{padded}\x1b[0m"),
    "Waiting" => format!("\x1b[33m{padded}\x1b[0m"),
    _ => padded,
};
// format string から :<7 を削除
```
heur + det の 2 箇所を修正。

### Gate evidence
- `just verify`: 713 tests PASS
- `just e2e-contract`: 6 passed, 0 failed
- `test-waiting-states.sh`: WaitingApproval・WaitingInput 両方の状態到達を実証してから assertions

---

## 2026-02-28 — E2E coverage向上 (test-freshness-fallback, test-error-state, evidence_mode in online scenarios)

### 追加: contract/test-freshness-fallback.sh

**カバー内容**: DOWN_THRESHOLD (15s) 経過後に `evidence_mode` が `"deterministic"` → `"heuristic"` に切り替わる契約。
resolver.rs Step 4: `Freshness::Stale|Down → winner_tier = EvidenceTier::Heuristic`。
`tick_freshness()` は `evidence_mode` のみ変更し `presence` は変えない。
Phase 4 でも検証: 再 inject → `"deterministic"` に戻ることを確認。

### 追加: contract/test-error-state.sh

**カバー内容**: `lifecycle.error` → `ActivityState::Error` 状態の生成・表示・JSON passthrough。
3 シナリオ: Error 初期到達 / Running→Error 遷移 / Error→Running 回復。
`display_state` の `other => other` branch で "Error" はそのまま表示（WaitingApproval と異なり正規化なし）。

### 追加: evidence_mode=deterministic check in online scenarios

- `multi-agent-same-session.sh`: 2 pane 両方に `evidence_mode=deterministic` 確認追加
- `same-cwd-multi-pane.sh`: 2 pane 両方に `evidence_mode=deterministic` 確認追加  
- `provider-switch.sh`: PROVIDER_A Running / PROVIDER_B Running の両フェーズに `evidence_mode=deterministic` 確認追加
- `single-agent-lifecycle.sh`: 既存の `evidence_mode=deterministic` 確認そのまま維持

### Gate evidence
- `just e2e-contract`: **8 passed, 0 failed** (6→8 tests)
- freshness test 実測: inject 停止後 11s で `"heuristic"` に切り替わりを確認 (DOWN_THRESHOLD=15s 以内)

---

## 2026-02-28 — CLI 全体再設計決定 (T-139 拡張)

### 背景

T-139 は当初 `--context=auto|off|full` フラグの追加として計画されていたが、
3案のコンペレビュー（opus × 3: Minimal/Density/Workflow）を経て、ユーザー合意の下で
「CLI 全体の再設計」に昇格。後方互換不要（現ユーザー=開発者のみ）。

### 採用設計 (コンペ結果の統合)

| 決定事項 | 採用元 | 内容 |
|----------|--------|------|
| bare `agtmux` = hierarchical tree | A/B案 | 全体構造把握。C案の triage は `agtmux ls --flat` 相当 |
| `agtmux ls --group=session\|pane` | B案 | 粒度選択フラグで list-* 3コマンドを統合 |
| `agtmux pick` 組み込み | C案 | fzf picker を 1st class コマンドに |
| `agtmux watch` | C案 | htop 風ライブダッシュボード |
| `agtmux wait` | C案 | `--idle`/`--no-waiting` でブロック待機 |
| `agtmux bar --tmux` | C案 | tmux カラーコード専用フラグ |
| `agtmux json` 分離 | C案 | 人間向けコマンドから `--json` を完全排除 |
| cwd = 末尾2セグメント | 独自 | worktree 環境での長パス問題を解決 |
| branch = `[branch]` ASCII括弧 | 共通 | 環境依存なし。`--icons` で Nerd Font opt-in |

### 廃止コマンド
`list-panes` / `list-windows` / `list-sessions` / `tmux-status` / `status`

### タスク分解
- T-139a: CLI Core (コマンド骨格 + ls + triage) — **実装開始**
- T-139b: Navigation (pick) — blocked_by T-139a
- T-139c: Monitor (watch + bar) — blocked_by T-139a
- T-139d: Script (wait + json) — blocked_by T-139a

---

## 2026-02-28 — T-139a: CLI Core 実装完了

### 設計変更の主な判断

**client-side git branch resolution を選択**:
daemon の hot path (poll_loop.rs) で `git rev-parse` を実行するとブロッキングリスクがあるため、
CLI 側で unique CWD ごとに非同期実行する方式を採用。
`server.rs` は `"git_branch": null` のプレースホルダーを返すのみ。

**bare `agtmux` = `Ls(default)`**:
`Option<Command>` + `subcommand_required = false` で bare invocation を `ls` にフォールバック。

### 新規ファイル
- `context.rs`: `short_path`, `git_branch_for_path`, `truncate_branch`, `consensus_str`, `build_branch_map` 等
- `cmd_ls.rs`: `format_ls_tree` / `format_ls_session` / `format_ls_pane` / `cmd_ls`

### テスト
- 新規: context 11件 + cmd_ls 24件 + client(bar) 6件 = 41件追加
- 削除: 旧 `format_panes/format_windows/format_sessions` ~28件
- 純増: +13件, 711 → 724 tests

### Gate evidence
- `just verify`: **724 tests PASS**

---

## 2026-02-28 — T-139b/c/d: CLI Navigation / Monitor / Script 実装完了

### T-139b: `agtmux pick`
- `cmd_pick.rs` 新規: `format_pick_candidates`, `cmd_pick`
- `fzf` 検出 (`which fzf`) → stdin pipe → stdout parse → `tmux switch-client -t {pane_id}`
- `--dry-run`: fzf 起動なし、候補一覧のみ表示
- `--waiting`: WaitingInput/WaitingApproval pane のみ表示
- 3 new tests

### T-139c: `agtmux watch`
- `cmd_watch.rs` 新規: ANSI `\x1b[2J\x1b[H` クリア + `format_ls_tree` ループ
- `tokio::signal::ctrl_c()` で Ctrl-C 終了
- `--interval N` (秒): デフォルト 2s
- crossterm 追加依存なし
- 2 new tests

### T-139d: `agtmux wait` + `agtmux json`
- `cmd_wait.rs` 新規: `WaitCondition { Idle, NoWaiting }`, `condition_met()`, exit code 0/1/2/3
  - `--idle`: 全 managed pane が Idle/Error/Unknown になるまで待機
  - `--no-waiting`: WaitingInput/WaitingApproval pane がゼロになるまで待機
  - `--session`: セッション名フィルタ; `--timeout`: タイムアウト秒; `--quiet`: 進捗非表示
  - `\r` progress line (tty 判定)
  - 8 new tests
- `cmd_json.rs` 新規: schema v1 `{version:1, panes:[...]}`, normalize helpers
  - `normalize_activity_state`: `"WaitingApproval"` → `"waiting_approval"` 等
  - `normalize_provider`: `"ClaudeCode"` → `"claude"` 等
  - `--health`: daemon 疎通確認のみ
  - 14 new tests

### `cli.rs` + `main.rs` 更新
- `LsOpts`, `BarOpts`, `PickOpts`, `WatchOpts`, `WaitOpts`, `JsonOpts` 全 opts 確定
- `main.rs`: `Wait` コマンドのみ `std::process::exit(exit_code)` で精密 exit code

### Gate evidence
- `just verify`: **751 tests PASS** (724 → 751, 純増 +27)

---

## 2026-02-28 — T-140: E2E Contract Script CLI Migration

### 背景
T-139 CLI 再設計で `list-panes --json`, `list-windows`, `list-sessions` 等が廃止された。
Review B-1 で指摘：E2E コントラクトスクリプトがこれらの廃止コマンドを直接呼び出しており `just e2e-contract` が壊れる状態だった。

### 変更内容

| ファイル | 変更内容 |
|---------|---------|
| `harness/common.sh` | `jq_get`: `list-panes --json` → `agtmux json`, `.[]` → `.panes[]` / debug も同様 |
| `test-schema.sh` | JSON schema v1 検証に変更（`type == "object"`, `.panes | type == "array"`, snake_case VALID_STATES） |
| `test-waiting-states.sh` | `list-windows` → `agtmux ls` / `list-sessions` → `agtmux ls --group=session` / activity_state 期待値 → snake_case |
| `test-error-state.sh` | `list-windows` → `agtmux ls` / activity_state → snake_case |
| `test-list-consistency.sh` | JSON ground truth: `list-panes --json` → `agtmux json` + `.panes[]` jq path / human views → `agtmux ls` |
| `test-multi-pane.sh` | `list-sessions` → `agtmux ls --group=session` / activity_state → snake_case |
| `test-freshness-fallback.sh` | activity_state "Running" → "running" |
| `test-claude-state.sh` / `test-codex-state.sh` | activity_state → snake_case |

### 設計メモ
- `presence` ("managed"/"unmanaged") と `evidence_mode` ("deterministic"/"heuristic"/"none") は schema v1 でも **変化なし**
- `activity_state` のみ snake_case 正規化: "Running" → "running", "WaitingApproval" → "waiting_approval" 等
- `wait_for_agtmux_state` の期待値が snake_case になったことで、provider-agnostic な detection E2E (Layer 3) も `jq_get` 経由なら自動的に恩恵を受ける

### Gate evidence
- `bash -n` syntax check: **10 scripts PASS**
- `just verify`: **751 tests PASS** (Rust unit tests 変化なし)

---

## 2026-03-01 — OSC シーケンス調査と Phase 8 Sources Enhancement 方針決定

### 背景

Claude Code の OSC シーケンス実態を調査し、2 つの外部レポート（agtmux-architecture-proposal.md、claude_osc_agtmux_assessment_2026-03-01.md）を評価した。
調査・評価内容: hooks 新イベント群、JSONL 新 record types、OSC 9;4 via pipe-pane、fd-based JSONL discovery。

### 調査で確認された Claude Code OSC シーケンス

| Sequence | 内容 | agtmux での利用可否 |
|----------|------|------------------|
| OSC 9;4 | Progress bar (state=3: thinking, state=0: done) | ✅ pipe-pane 経由で取得可（Post-MVP） |
| OSC 9 | Desktop notification | △ tmux passthrough 要 |
| OSC 2/0 | Terminal title (`/rename` 時のみ) | tmux `pane_title` 変数経由でアクセス可 |
| OSC 8 | Clickable hyperlinks | ✗ tmux 内 broken、検出用途なし |
| **OSC 133** | **Shell integration** | **✗ Claude Code が emit しない（issue #26235: open FR）** |

### 外部レポートの評価

**Architecture Proposal の問題点**:
- OSC 133 を「Claude Code が emit するシーケンス」として rank-0 に置いているが、**これは事実誤認**。
  OSC 133 は bash/zsh/fish の shell integration スクリプトが emit するもの。Claude Code 自体は emit しない。
  GitHub issue #26235 が「OSC 133 を実装してほしい」という open feature request として存在する（＝現在は出ていない）。
- `pipe-pane` 実験で OSC 133 を取得できたとする結果は、pane 内の shell integration スクリプトによるものと推定。
- fd-based discovery は本物の洞察。Hooks deprecated 方針は premature。

**OSC Assessment Report**: 技術的に正確。OSC 133 が open feature request であることを正しく把握。保守的だが妥当。

### 合意した方針（Phase 8 Sources Enhancement）

1. **Hooks は維持・拡張**
   - 新 hook events 追加（SessionEnd, PermissionRequest, SessionStart+transcript_path, UserPromptSubmit, PostToolUseFailure, PreCompact）
   - `agtmux setup-hooks` でゼロ摩擦化（手動編集不要）
   - Hooks が不可欠な理由: WaitingApproval の deterministic 化、SessionEnd による 15s void 解消、transcript_path による JSONL direct binding

2. **JSONL: fd-based discovery を parallel path として追加**
   - Priority: transcript_path (P1) → fd-based/lsof/procfs (P2) → CWD-based (P3, existing fallback)
   - 同一 CWD の複数 pane 問題を根本解決

3. **OSC 9;4 via pipe-pane を Post-MVP で optional semi-deterministic source として追加**
   - rank: hooks(0) > jsonl(1) > osc_tap(2) > poller(3)
   - capability-gated（tmux 3.3+、pipe-pane 先占確認）
   - OSC 不在は negative evidence に使用しない

4. **OSC 133 は採用しない**（issue #26235 が close されるまで）

### 既実装の確認

調査中に以下が Phase 8 相当の実装として既に完了していることを確認:
- `progress` record → `"activity.running"` 変換: JSONL source 実装済み
- `custom-title` 抽出: T-135b で実装済み
- `setup-hooks` CLI command: T-112 で実装済み

### 関連 docs 更新（本日）

- `docs/30_architecture.md`: C-017 `agtmux-source-osc-tap` [Post-MVP] 追加
- `docs/20_spec.md`: FR-053〜FR-060 追加、FR-030 に Post-MVP rank 追記
- `docs/40_design.md`: 新 hooks 一覧、3-tier JSONL discovery、OSC Tap セクション追加
- `docs/60_tasks.md`: Phase 8 タスク T-E01〜T-E04 追加
- `docs/80_decisions/ADR-20260301-osc-architecture.md`: 新規作成

## 2026-03-01 — Phase 8 T-E01〜T-E03 実装完了

### 概要

Phase 8 Sources Enhancement の MVP タスク（T-E01、T-E02、T-E03）を実装。
Review Pack: `docs/85_reviews/RP-TE01-TE02-TE03-sources-enhancement.md` → verdict: **GO_WITH_CONDITIONS**
（2 条件 = T-E03a / T-E03b として follow-up タスク登録済み、次 PR で対応）

### 変更内容

| タスク | ファイル | 変更内容 |
|--------|---------|---------|
| T-E01 | `translate.rs` | 6 new hook type mappings (SessionStart/End/PermissionRequest/UserPromptSubmit/PostToolUseFailure/PreCompact) + 6 test cases |
| T-E01 | `setup_hooks.rs` | HOOK_TYPES 5→11 |
| T-E01 | `discovery.rs` | `discovery_from_transcript_path()` 追加、`session_id_from_jsonl_path` を public 化 |
| T-E01 | `poll_loop.rs` | `DaemonState.transcript_path_hints` 追加、Step 8b で SessionStart/End をスキャン、Step 6b で P1 overlay |
| T-E02 | `discovery.rs` | `PaneDiscoveryHint` struct 追加、`discover_jsonl_via_lsof()` 追加（P2 tier）、`discover_sessions()` signature 更新 |
| T-E02 | `source.rs` | `discover_sessions` call site 更新 |
| T-E02 | `poll_loop.rs` | Step 6b を `Vec<PaneDiscoveryHint>` に更新、pane_pid を populate |
| T-E03 | `cli.rs` | `SetupHooksOpts` に `--check` フラグ追加 |
| T-E03 | `setup_hooks.rs` | `HookStatus`、`HookCheckResult`、`check_hooks()` 追加 |
| T-E03 | `main.rs` | SetupHooks arm で `opts.check` 分岐 |

### Gate 証跡

- `just verify`: PASS（760+ tests, 0 failed）
- `cargo fmt --check`: PASS
- `cargo clippy -D warnings`: PASS

### Discovery priority 確定

```
P1: transcript_path (SessionStart hook payload) — poll_loop.rs Step 6b overlay
P2: fd-based (lsof -F n -p {pane_pid})         — discover_sessions 内、P3 の前
P3: CWD-based (sessions-index + latest .jsonl)  — discover_sessions フォールバック
```

---

## 2026-02-28 — T-135b: Claude JSONL Conversation Title Extraction

### 概要
Claude Code が JSONL ファイルに書き込む `custom-title` イベントから会話タイトルを抽出し、
`DaemonState.conversation_titles` に格納。T-135a (Codex) と同じ map を使うため `server.rs` 変更不要。

### 変更内容

| ファイル | 変更内容 |
|---------|---------|
| `translate.rs` | `ClaudeJsonlLine` に `custom_title: Option<String>` 追加、`timestamp` を `Option<>` 化 |
| `watcher.rs` | `SessionFileWatcher` に `last_title: Option<String>` + `last_title()`/`set_title()` 追加 |
| `source.rs` | `poll_files()` で `custom-title` 行を検出 → `watcher.set_title()` → `continue` |
| `poll_loop.rs` | `poll_files()` 直後に discoveries を走査し `st.conversation_titles[session_id] = title` |

### 設計メモ
- `custom-title` イベント: `{"type":"custom-title","customTitle":"タイトル","sessionId":"uuid"}`
- セッション中に複数回出現 → 最後の値が現在タイトル（watcher が上書き）
- 空文字列は `if !title.is_empty()` でスキップ
- borrow checker 制約: Vec 収集 → insert パターン（`claude_jsonl_watchers` 不変 + `conversation_titles` 可変の共存）
- pane watcher 差し替え時（inode 変更）は `new()` で `last_title: None` リセット → 新 JSONL の custom-title まで null

### Review summary
- Reviewer 1 (codex-style): GO_WITH_CONDITIONS → 条件修正後 GO
  - C-1: コメント修正（sessions-index.json → custom-title JSONL events） ✅
  - C-2: 空文字列スキップテスト追加 ✅
- Reviewer 2 (Claude): GO（blocking issues なし）
- Orchestrator: **GO**

### Gate evidence
- `just verify`: **754 tests PASS** (751 → 753 → 754, +3 new tests)
  - `custom_title_field_deserialized_from_custom_title_line` (translate.rs)
  - `poll_files_extracts_custom_title_from_jsonl` (source.rs)
  - `poll_files_ignores_empty_custom_title` (source.rs)

---

## 2026-03-01 — T-135b Phase 2: Codex conversation_title (JSONL scanner) + e2e test scenarios

### 概要
- `codex_poller.rs`: `read_jsonl_session_meta` を修正して Codex JSONL から実際のユーザータスクテキストを
  `conversation_title` として抽出する。
- `daemon.sh`: e2e テスト用デーモン起動スクリプトを修正して、グローバルインストールの stale binary を
  使わず常にローカルビルドのバイナリを使うよう修正。
- 新規 e2e シナリオ: `codex-title.sh`、`claude-title.sh`

### 問題と修正

#### 1. Codex JSONL の session_meta にタスクテキストがない
`session_meta.payload.instructions` は system prompt（base_instructions）であり、ユーザータスクではない。
実際のタスクは JSONL 6行目の `response_item role=user` にある。

**修正**: `read_jsonl_session_meta` で session_meta 後30行をスキャンし、`role=user` かつ
`#`（AGENTS.md インジェクション）または `<`（XML コンテキストブロック）で始まらない
最初の content text を `instructions` として採用。120 文字にトリミング。

```
JSONL レイアウト:
  line 1: session_meta  (cwd, base_instructions)
  line 2: response_item role=developer  (permissions/sandbox)
  line 3: response_item role=user       (AGENTS.md → skip #で始まる)
  line 4: event_msg     task_started
  line 5: response_item role=developer  (collaboration mode)
  line 6: response_item role=user       ← 実際のタスクテキスト ✓
```

#### 2. e2e テストが stale なグローバルバイナリを使用
`common.sh` が `AGTMUX_BIN="agtmux"` とデフォルト設定する。`daemon.sh` の古い判定
`[ "${AGTMUX_BIN:-}" != "" ]` は "agtmux" (コマンド名) を真とみなし、
`command -v agtmux` がグローバルの `/Users/virtualmachine/go/bin/agtmux` (2月28日ビルド、
JSONL スキャナーなし) を発見 → デーモンが stale binary で起動 → JSONL スキャナーなし → e2e 失敗。

**修正 1**: `daemon.sh`: `AGTMUX_BIN` が絶対パス (`/* `) のときのみ pre-built として採用。
それ以外は `cargo build` してローカルの `target/debug/agtmux` を使用。

**修正 2**: デーモン起動後に `AGTMUX_BIN` を解決済みバイナリパスで上書きし、
`jq_get`/`wait_for_agtmux_state` がデーモンと同一バイナリを使うことを保証。

### 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `codex_poller.rs` | `read_jsonl_session_meta`: 30行スキャン for ユーザータスク抽出; `JsonlSessionMeta.instructions` doc 更新 |
| `daemon.sh` | 絶対パス判定 + `AGTMUX_BIN` 上書き (Codex review P1.1 対応) |
| `scenarios/codex-title.sh` | 新規: Codex conversation_title e2e テスト |
| `scenarios/claude-title.sh` | 新規: Claude custom-title back-to-back injection + 持続確認 e2e テスト |

### Gate evidence
- `just verify`: PASS (fmt + clippy + tests)
- e2e `codex-title.sh`: PASS
- e2e `claude-title.sh`: PASS
- e2e `single-agent-lifecycle.sh` (claude, codex): PASS
- e2e `provider-switch.sh`: PASS
- e2e `multi-agent-same-session.sh` (claude, codex): PASS

### Codex review 判定
- P1.1 (AGTMUX_BIN 未同期): **修正済み** → daemon.sh で `AGTMUX_BIN="$_built_bin"` 上書き
- P1.2 (JSONL_ACTIVE_THRESHOLD_SECS=25 で sleep 60 中に idle 誤分類): **Pre-existing** — 今回変更外。
  将来 Codex ロングタスク対応が必要な場合は JSONL_ACTIVE_THRESHOLD_SECS の引き上げを検討。
- P2 (notLoaded stale bindings): **Pre-existing** — App Server client path で今回変更外。
- Orchestrator 判定: **GO**

## 2026-03-05 — agtmux-term V2 最終案採択（A0先行）+ Orchestrator handover

### 決定
- 既存レビュー（codex/claude）を統合し、実装順序を A0/A1/A2 に再スコープ。
- **A0（UX fix）を最優先**: inventory-first + metadata non-blocking overlay を cross-repo で実装する。

### 採択理由
- ユーザー体感問題（初回表示遅延、sidebar空白、metadata timeout影響）は、
  protocol刷新より先に `local UI blocking` を除去することで最短改善できるため。

### 反映
- handover doc 作成:
  - `docs/85_reviews/RP-20260305-agtmux-term-v2-a0-handover.md`
- 最終案（term側出力）:
  - `/tmp/agtmux-v2-final-plan-20260305-v3.md`

### Next
- daemon/runtime: cached snapshot即時返却 + non-destructive metadata failure semantics 実装
- term: inventory/metadata lane分離（A0）を実装

## 2026-03-05 — agtmux-term V2 A0 closeout + daemon A1 handover同期

### 状態同期
- cross-repo A0 は完了として閉じる。
  - daemon baseline: `09722b7` (`feat: add A0 inventory-first cached snapshot and metadata backoff`)
  - term baseline: `5c5ea10` (`feat: implement A0 inventory-first local fetch and snapshot compatibility`)
- `docs/60_tasks.md` の `T-XTERM-A0` を DONE 扱いに更新。

### docs反映
- `docs/20_spec.md`
  - FR-061 / FR-062 を追加し、A1 の `ui.bootstrap.v2` / `ui.changes.v2` 契約と explicit resync を固定。
- `docs/40_design.md`
  - Appendix A9 を追加し、epoch/seq ownership、`resync_required`、A1/A2 の責務分界を明文化。
- `docs/60_tasks.md`
  - `T-XTERM-A1` を追加。daemon 側の次作業を protocol contract 固定に限定した。
- `docs/90_index.md`
  - Cross-repo V2 A1 handover 導線を追加。

### scratch handover
- `/tmp/agtmux-v2-daemon-a1-handover-20260305.md` を parallel implementation 用に作成。
- ただし source of truth は引き続き `docs/20_spec.md` / `docs/40_design.md` / `docs/60_tasks.md`。

### Next
- daemon 側は `T-XTERM-A1` として `ui.bootstrap.v2` / `ui.changes.v2` の wire contract を固定する。
- A2（ack compaction / true stream / observability）は A1 完了後に着手する。

## 2026-03-06 — agtmux-term V2 A1 daemon contract complete

### 実装
- `crates/agtmux-daemon-v5/src/projection.rs`
  - replay cursor (`epoch`, `seq`) と strict replay validator を追加。
  - `resync_required { current_epoch, latest_snapshot_seq, reason }` を projection レベルで返すようにした。
  - change log は mutable current-state 参照ではなく、その seq 時点の pane/session snapshot を保持するようにした。
  - `tick_freshness()` 由来の pane/session evidence-mode 変更も change log に記録するよう修正した。
- `crates/agtmux-runtime/src/server.rs`
  - `ui.bootstrap.v2` を追加し、`epoch`, `snapshot_seq`, `panes`, `sessions`, `generated_at`, `replay_cursor` を返すようにした。
  - `ui.changes.v2` を追加し、normal replay と `resync_required` を分岐返却するようにした。

### 検証
- `cargo test -p agtmux-daemon-v5` → 151 passed
- `cargo test -p agtmux` → 160 passed

### 状態
- `docs/60_tasks.md` の `T-XTERM-A1` を DONE 扱いに更新。
- A2（ack compaction / true stream / observability）は未着手のまま維持。

## 2026-03-06 — agtmux-term V2 A2 daemon-side kickoff (replay ack compaction + `ui.health.v1`)

### 実装
- `crates/agtmux-daemon-v5/src/projection.rs`
  - sync-v2 専用 replay log を追加し、legacy `changes` log と retention を分離した。
  - `ui.bootstrap.v2` / `ui.changes.v2` cursor を implicit ack として扱えるようにし、acked seq まで replay log を compact するようにした。
  - replay observability snapshot (`current_epoch`, `cursor_seq`, `head_seq`, `lag`, `last_resync_reason`, `last_resync_at`) を追加した。
- `crates/agtmux-runtime/src/poll_loop.rs`
  - runtime health 用に `runtime_started_at`, `runtime_last_ok_at`, `runtime_last_error` を追加した。
  - focus health 用に `focused_pane_id`, `focus_mismatch_count`, `focus_last_sync_at` を追加し、tmux active-pane invariant から更新するようにした。
- `crates/agtmux-runtime/src/server.rs`
  - additive JSON-RPC `ui.health.v1` を追加した。
  - `runtime` / `replay` / `overlay` / `focus` status を daemon 側で計算して返すようにした。
  - `ui.bootstrap.v2` / `ui.changes.v2` 呼び出し時に replay ack / resync observability を更新するようにした。

### ドキュメント
- `docs/20_spec.md`
  - FR-063 / FR-064 を追加し、`ui.health.v1` と sync-v2 replay ack compaction の不変条件を固定した。
- `docs/40_design.md`
  - Appendix A10 として `ui.health.v1` payload と dedicated replay retention の設計を追加した。
- `docs/60_tasks.md`
  - `T-XTERM-A2` を追加し、daemon 側実装の証跡と cross-repo 残課題を記録した。

### 検証
- `cargo test -p agtmux-daemon-v5` → 153 passed
- `cargo test -p agtmux` → 165 passed

### 状態
- daemon 側 A2 kickoff は handover scope に沿って実装開始済み。
- cross-repo の残りは agtmux-term 側 `ui.health.v1` consumer 接続確認と A1 compatibility handback。

## 2026-03-07 — agtmux-term live smoke で A1 bootstrap wire drift を確認

### 発見
- `AGTMUX_BIN=/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux swift run AgtmuxTerm` で local daemon 自体は起動するが、`ui.bootstrap.v2` decode が `sync-v2 pane payload contains legacy identity field 'session_id'` で fail-closed することを確認した。
- 根本原因は `crates/agtmux-runtime/src/server.rs` の `build_ui_bootstrap_v2()` が human-facing inventory serializer `build_pane_list()` を再利用している点。`build_pane_list()` は legacy JSON / CLI 向けに `session_id` を保持しており、strict sync-v2 consumer にそのまま漏れていた。
- 影響範囲は bootstrap compatibility handback で、A2 observability 自体ではない。現状の agtmux-term では overlay lane が無効化され、`Local daemon incompatible` banner が出続ける。

### docs反映
- `docs/20_spec.md`
  - FR-065 / FR-066 を追加し、`ui.bootstrap.v2.panes[]` required exact fields と legacy identity alias 禁止を明文化した。
- `docs/40_design.md`
  - Appendix A9 に sync-v2 専用 DTO / builder 分離、forbidden fields、producer boundary を追記した。
- `docs/60_tasks.md`
  - `T-XTERM-A3` を追加し、TDD -> builder split -> live smoke の順で compatibility handback を回収する実装計画を登録した。
  - `T-XTERM-A2` は `T-XTERM-A3` 依存を追加した。

### Next
- `T-XTERM-A3` として failing regression を先に追加し、`ui.bootstrap.v2` serializer を `build_pane_list()` から切り離す。
- `just verify` 後に strict agtmux-term consumer で live smoke を再実施する。

## 2026-03-08 — Cross-repo live E2E ownership split locked with agtmux-term

### 背景
- live activity-state / waiting-state / no-bleed は `agtmux-term` 側でも重要だが、semantic truth 自体は daemon が生成している。
- 同じ real-provider scenario を両 repo に無秩序に複製すると、producer-side regression と consumer-side regression の責任境界が曖昧になる。

### docs反映
- `docs/20_spec.md`
  - FR-067 を追加し、real-CLI semantic live E2E の source of truth は agtmux repo が持つことを固定した。
  - cross-repo validation は「semantic truth vs consumer truth」の責務分離で行うことを NFR-Reliability に追記した。
- `docs/40_design.md`
  - producer boundary に live E2E ownership を追記し、agtmux-term 側の mirror は thin canary に限定する方針を明文化した。
- `docs/60_tasks.md`
  - `T-XTERM-A4` を追加し、Claude Sonnet 4.6 / Codex 5.4 medium の live scenario handback を daemon-owned source-of-truth suite として追跡することにした。

### 状態
- agtmux-term 側は daemon payload truth を primary oracle にする thin live canary を実装開始できる状態になった。
- daemon 側の semantic truth suite は引き続き agtmux repo が owner。

## 2026-03-08 — T-XTERM-A4 docs-first handover published

### docs反映
- `docs/85_reviews/RP-20260308-agtmux-term-semantic-truth-handover.md`
  - semantic truth ownership split を handover 文書として恒久化した。
  - daemon-owned scenario matrix (`provider`, `presence`, `running`, completion, `waiting_input`, `waiting_approval`, `conversation_title`, no-bleed) を scenario script と紐付けた。
  - provider-specific live prompt guidance を Claude Sonnet 4.6 / Codex 5.4 medium で固定した。
  - `just preflight-online` の explicit gate（tmux/CLI/auth/network）を明記した。
  - agtmux-term は boundary assertions のみを mirror する方針を明文化した。
- `docs/60_tasks.md`
  - `T-XTERM-A4` に handover doc 参照を追加した。

### 状態
- `T-XTERM-A4` の docs-first deliverable は満たした。
- cross-repo strict consumer smoke の最終閉塞は `T-XTERM-A3` 完了後に継続する。

## 2026-03-08 — T-XTERM-A3 reopened on dirty persistent-state bootstrap drift

### 発見
- fresh live app evidence from `agtmux-term` shows the earlier `session_id` fix was not sufficient on the shipped app-managed daemon path:
  - `ui.bootstrap.v2` still fails strict consumer decode in the normal app path
  - direct socket inspection of `/Users/virtualmachine/Library/Application Support/AGTMUXDesktop/agtmuxd.sock` shows 94 panes total, with 88 managed panes carrying `session_name: null` and `window_id: null`
- the failure is producer-side:
  - `build_sync_v2_pane_list()` is emitting managed rows even when live tmux inventory can no longer resolve exact location for those rows
  - strict agtmux-term consumer correctly rejects the whole bootstrap epoch once any such row appears

### なぜ online E2E が素通りしたか
- existing daemon online/e2e scenarios create a fresh daemon on a temporary socket with fresh tmux state
- they verify semantic truth for live provider activity, but they do not simulate dirty persistent daemon state with orphan managed rows left over from prior sessions
- therefore the producer remained green in clean-socket tests while the persistent app-managed socket still served invalid strict-consumer bootstrap rows

### docs反映
- `docs/20_spec.md`
  - FR-065 now explicitly forbids emitting managed sync-v2 panes with null exact-location fields; unresolved rows must be excluded or repaired before bootstrap emission
- `docs/60_tasks.md`
  - `T-XTERM-A3` now tracks the reopened root cause and the dirty-state regression gap
  - the scratch handover for this reopened slice is `/tmp/agtmux-bootstrap-null-exact-location-handover-20260308.md`

### Next
- add failing producer-side regression coverage for orphan managed rows with unresolved exact location
- change sync-v2 bootstrap emission so such rows are not emitted as managed panes with null required fields
- rerun:
  - clean online/e2e suite
  - cross-repo live smoke against the persistent app-managed socket used by agtmux-term

## 2026-03-08 — T-XTERM-A3 producer-side null exact-location fix landed

### 実装
- `crates/agtmux-runtime/src/server.rs`
  - `build_sync_v2_pane_list()` now excludes managed panes when live tmux inventory cannot resolve the pane's exact location
  - the serializer no longer emits sync-v2 managed rows with `session_name: null` / `window_id: null`
- added producer-side regression coverage:
  - `ui_bootstrap_v2_excludes_managed_pane_when_exact_location_is_unresolved`
  - tightened required-field regression to assert non-null `session_name` / `window_id`

### 検証
- `cargo test -p agtmux ui_bootstrap_v2_`
- `cargo test -p agtmux`

### 残り
- dirty persistent app-managed socket での cross-repo live smoke
- 必要なら online/e2e に dirty-state bootstrap exact-location scenario を追加

## 2026-03-08 — T-XTERM-A5 opened on managed-exit semantic truth drift

### Fresh cross-repo repro
- agtmux-term side reran a fresh real tmux + real Codex repro against a temp daemon socket:
  - launched `codex exec` from a plain `zsh -l` pane
  - force-terminated the pane child processes
  - verified the pane had already returned to `current_cmd=zsh`
- producer truth still remained stale on that exact row:
  - `presence=managed`
  - `provider=codex`
  - `activity_state=waiting_input`
  - `evidence_mode=heuristic`

### Why current producer suite missed it
- current online/e2e validates semantic truth for entry, running/completion, waiting states, title, and no-bleed
- it does not yet force a managed pane back to a plain shell and require exact-row demotion to `presence=unmanaged`
- current confirmed evidence is Codex-only; Claude remains a follow-up validation item rather than a proven repro for this slice

### docs反映
- `docs/20_spec.md`
  - FR-068 added: stale managed/provider truth must not survive once a pane has returned to shell
- `docs/60_tasks.md`
  - `T-XTERM-A5` added for producer-side managed-exit semantic truth recovery
  - scratch handover path recorded as `/tmp/agtmux-managed-exit-semantic-truth-handover-20260308.md`

### Next
- add a failing producer-side managed-exit regression first
- make the semantic truth reducer demote exact rows back to unmanaged shell truth after agent exit / forced termination
- rerun producer online/e2e plus cross-repo smoke from agtmux-term
- after the generic demotion path is fixed, add the same follow-up validation for Claude

## 2026-03-08 — T-XTERM-A5 producer-side shell demotion fix landed

### 実装
- `crates/agtmux-daemon-v5/src/projection.rs`
  - added `demote_panes_to_unmanaged()` so an exact pane row can be removed from managed projection state once live tmux truth has returned to a plain shell
  - pane/session removal now clears resolver state, session-to-pane links, and records explicit change-log removals
- `crates/agtmux-runtime/src/poll_loop.rs`
  - after freshness downgrade, shell rows (`zsh`, `bash`, `fish`, `sh`, `csh`, `tcsh`, `ksh`, `dash`, `nu`, `pwsh`) now win over stale heuristic managed truth
  - when tmux reports a shell row and the projection has already fallen back to heuristic evidence, the pane is demoted back to unmanaged shell truth and transcript hints are cleared
- producer regression and live coverage added:
  - `demote_panes_to_unmanaged_removes_exact_row_and_session_state`
  - `poll_tick_demotes_managed_pane_after_return_to_shell`
  - `poll_tick_demotes_stale_deterministic_pane_after_return_to_shell`
  - new real-CLI scenario `scripts/tests/e2e/scenarios/managed-exit.sh`
- online/e2e completion/title scenarios were updated to accept both valid post-completion outcomes:
  - managed completion (`waiting_input` / `idle`)
  - exact shell demotion (`presence=unmanaged`, `provider=null`, `activity_state=null`)
- `scripts/tests/e2e/harness/daemon.sh`
  - fixed stale local binary reuse so repo-local `target/debug/agtmux` / `target/release/agtmux` is rebuilt before daemon launch unless `AGTMUX_SKIP_BUILD=1`

### 検証
- `cargo test -p agtmux-daemon-v5`
- `cargo test -p agtmux`
- `PROVIDER=codex bash scripts/tests/e2e/online/run-all.sh`
- `swift test -q --filter AppViewModelA0Tests/testManagedExitChangeClearsStaleProviderActivityAndTitleOnNextPublish` in `agtmux-term`
- `AGTMUX_LIVE_TEST_BIN=/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux swift test -q --filter AppViewModelLiveManagedAgentTests` in `agtmux-term`

### 残り
- Claude real-CLI managed-exit follow-up validation

## 2026-03-08 — T-XTERM-A5 Claude follow-up validation completed

### 実装
- `scripts/tests/e2e/scenarios/managed-exit.sh`
  - provider-specific sourcing was removed; the scenario now runs with `PROVIDER=claude` and `PROVIDER=codex`
  - phase-1 entry gate now requires `presence=managed` plus a live child process under the pane shell, which matches the forced-termination seam better than a provider-specific `activity_state=running` requirement
- `scripts/tests/e2e/online/run-all.sh`
  - Claude lane now includes `managed-exit`

### 検証
- `PROVIDER=claude bash scripts/tests/e2e/scenarios/managed-exit.sh`
- `PROVIDER=codex bash scripts/tests/e2e/scenarios/managed-exit.sh`
- `PROVIDER=claude bash scripts/tests/e2e/online/run-all.sh`

### 結果
- Claude real-CLI lane also demotes the exact row back to shell truth after forced termination:
  - `presence=unmanaged`
  - `provider=null`
  - `activity_state=null`
- `PROVIDER=claude bash scripts/tests/e2e/online/run-all.sh` → 7 passed, 0 failed

### 残り
- この slice に関する producer-side follow-up validation はなし

## 2026-03-08 — T-XTERM-A6 opened on app-launched explicit --tmux-socket drift

### Fresh cross-repo evidence from `agtmux-term`
- term-side metadata-enabled XCUITest now proves the remaining failure is not stale daemon reuse and not socket-name resolution:
  - the app launches:
    - `/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux --socket-path /Users/virtualmachine/.agt/uit-<token>.sock daemon --tmux-socket /private/tmp/tmux-501/agtmux-managed-<token>`
  - `UITestTmuxBridge` capture-pane proves a real Codex run completed in the target plain `zsh` pane on that isolated tmux server
  - same app process probing the custom daemon socket still gets:
    - `ui.bootstrap.v2 total=0 managed=0`
    - `probeTarget=nil`
    - daemon stderr empty
- standalone shell repro with the same local binary and exact daemon args sees the managed pane within 3 seconds

### Conclusion
- the remaining drift is producer-side and launch-context-specific:
  - explicit `--tmux-socket` works from a normal shell
  - explicit `--tmux-socket` fails when the daemon is spawned from the codesigned app/XCUITest path
- this opens `T-XTERM-A6`
- scratch handover published: `/tmp/agtmux-app-launched-explicit-tmux-socket-handover-20260308.md`

### Next
- add a failing producer-side repro for app-like explicit-`--tmux-socket` launch context
- fix the daemon so explicit `--tmux-socket` remains authoritative across shell and app-owned launches

## 2026-03-08 — T-XTERM-A6 refined after stripped-PATH fix and downstream env hardening

### What is now ruled out
- producer-side stripped-PATH drift is fixed:
  - `cargo build -p agtmux`
  - `cargo test -p agtmux-tmux-v5`
  - `bash scripts/tests/e2e/scenarios/explicit-tmux-socket-sanitized-path.sh`
  - `cargo test -p agtmux`
  - all green
- term-side child-daemon env hardening is also in place on the current downstream worktree:
  - `TMUX_BIN=/opt/homebrew/bin/tmux`
  - normalized `HOME`, `USER`, `LOGNAME`, `XDG_CONFIG_HOME`, `CODEX_HOME`, `PATH`

### Fresh downstream evidence
- rerun of the focused metadata-enabled UI lane still fails with the same producer-visible shape:
  - `capture-pane` proves the real Codex run completed in the target app-driven pane
  - same app process probing the custom daemon socket still gets:
    - `ui.bootstrap.v2 total=0 managed=0`
    - `probeTarget=nil`
    - daemon stderr empty
- downstream failure summary now explicitly shows the normalized child-daemon env, so the remaining drift is not explained by sparse PATH/user env alone

### Interpretation correction
- `ui.bootstrap.v2 total=0` here means sync-v2 emitted zero managed rows
- it does not, by itself, prove the daemon's tmux inventory is empty
- likely remaining surface is managed promotion / detection in the app-launched context, not raw tmux socket reachability alone

### Conclusion
- `T-XTERM-A6` remains open, but its scope is narrower:
  - explicit `--tmux-socket` works from shell repro
  - explicit `--tmux-socket` works under stripped PATH
  - explicit `--tmux-socket` still fails when the daemon is launched from the agtmux-term metadata-enabled app/XCUITest lane even after downstream env normalization
- next producer-side step should be a higher-fidelity app-launched repro or launch-context instrumentation, not more generic PATH hardening

## 2026-03-08 — T-XTERM-A6 Phase 1 landed as a failing higher-fidelity producer repro

### Implementation
- added `scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh`
  - daemon starts first under app-like normalized env with explicit `--tmux-socket <path>`
  - tmux server/session/pane are created only after daemon launch
  - the scenario fails unless the daemon later inventories the late-started pane on that explicit socket
- wired the new scenario into `scripts/tests/e2e/contract/run-all.sh`

### Verification
- `bash scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh`
- `bash scripts/tests/e2e/contract/run-all.sh`

### Result
- the new repro is intentionally red and captures the producer bug directly:
  - tmux side shows the late-started pane (`%0 zsh`)
  - daemon side still inventories `[]`
- contract suite result:
  - `11 passed, 1 failed`
  - only `explicit-tmux-socket-app-child-late-server.sh` is red

### Meaning
- `T-XTERM-A6` Phase 1 is now satisfied:
  - higher-fidelity producer-side repro exists
  - it fails without depending on online auth or downstream UI harness
- next step is producer-side fix / instrumentation against this new red

## 2026-03-09 — T-XTERM-A6 narrowed again: downstream fixed launch/daemon blockers and still gets zero managed rows

### Fresh downstream evidence
- rerun of the focused `agtmux-term` plain-zsh Codex UI lane now proves the downstream harness is no longer the blocker:
  - inventory-only launch no longer fails at `Running Background`
  - delayed metadata enable does spawn the isolated daemon on the custom socket
  - downstream failure summary now includes:
    - `daemonLaunch=spawned:/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux:--socket-path,/Users/virtualmachine/.agt/uit-<token>.sock,daemon,--tmux-socket,/private/tmp/tmux-501/agtmux-managed-<token>`
    - `daemonEnv=... TMUX_BIN=/opt/homebrew/bin/tmux ...`
    - `probe=ok total=0 managed=0`
    - `probeTarget=nil`
- the same downstream capture still proves a real Codex run completed inside the target plain `zsh` pane on that isolated tmux server

### Interpretation
- this is stronger than the earlier empty-socket hypothesis:
  - explicit `--tmux-socket` daemon startup succeeded
  - the custom daemon socket is reachable
  - the issue is that the daemon still promotes zero managed sync-v2 rows in that app-child explicit-socket context
- likely remaining producer-side surface:
  - plain `zsh` pane child-process discovery / promotion
  - specifically, `crates/agtmux-tmux-v5/src/capture.rs` currently returns early for `process_hint="shell"` and may never inspect the pane's child process tree in the explicit-socket app-child lane

### Conclusion
- `T-XTERM-A6` is now narrowed to producer-side managed promotion, not launch-context wiring
- next producer step should either:
  - add a failing producer test for shell-pane child-agent promotion under explicit `--tmux-socket`, or
  - fix `inspect_pane_processes_deep()` / related promotion code so shell panes can promote to managed when their child process tree clearly contains Codex/Claude

## 2026-03-09 — T-XTERM-A6 root cause tightened to tmux list-panes delimiter drift

### Fresh producer-side evidence
- `scripts/tests/e2e/scenarios/explicit-tmux-socket-shell-child-promotion.sh` now fails before promotion with:
  - `inventory fetch failed: failed to parse list-panes line 1: expected at least 11 tab-separated fields, got 1`
- direct tmux probing isolated the format drift:
  - normal shell:
    - `tmux -S <sock> list-panes -a -F "#{session_id}\t..."`
    - returns actual tab-delimited rows
  - app-like sanitized env (`env -i HOME=... PATH=... TMUX_BIN=... tmux -S <sock> list-panes -a -F "#{session_id}\t..."`)
    - returns `_`-delimited rows instead of tab-delimited output
  - the same sanitized env with a printable pipe delimiter:
    - `tmux -S <sock> list-panes -a -F "#{session_id}|#{session_name}|..."`
    - returns pipe-delimited rows unchanged

### Interpretation
- explicit `--tmux-socket` app-child drift is not only a managed-promotion problem
- `crates/agtmux-tmux-v5/src/pane_info.rs` relies on a tab-delimited `list-panes -F` contract that is not stable under app-like sanitized env
- this explains why the daemon can launch successfully yet still publish empty inventory / empty managed bootstrap in the app-child lane

### Next step
- change `list_panes()` to use a printable literal delimiter (`|`) and update parser/tests accordingly
- rerun:
  - `scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh`
  - `scripts/tests/e2e/scenarios/explicit-tmux-socket-shell-child-promotion.sh`
  - cross-repo `agtmux-term` focused metadata-enabled UI lane

## 2026-03-09 — T-XTERM-A6 handback refreshed after downstream empty-bootstrap hardening

### Fresh downstream evidence
- `agtmux-term` landed a real consumer-side fix:
  - `inventory present + ui.bootstrap.v2 panes=[]` is no longer treated as a ready sync-v2 epoch
  - focused integration regression is green on the term side
- focused downstream UI rerun now fails earlier and more precisely:
  - it waits for a non-empty isolated bootstrap before launching the live Codex proof
  - that readiness gate still times out with:
    - `probe=ok total=0 managed=0`
    - `probeTarget=nil`
    - visible app inventory row still present as unmanaged `zsh`
  - daemon stdout/stderr captured from the app child shows only:
    - `agtmux daemon starting`
    - `UDS server listening on /Users/virtualmachine/.agt/uit-<token>.sock`
  - same-process explicit-socket probe is now also captured:
    - `appDirectSocketProbe=agtmux-e2e-managed-<token>|@0|%0|zsh`
    - so the app process itself can run `tmux -S <resolved socket path> list-panes` and see the isolated pane
    - the divergence is now strictly daemon-side on that same explicit socket path

### Interpretation
- downstream no longer conflates:
  - launch activation
  - stale daemon reuse
  - empty-bootstrap consumer priming
- the remaining red is now fully on the producer side again:
  - same-user app process inventory sees the isolated tmux pane
  - same-user app process direct `tmux -S <resolved socket path>` probing also sees the isolated tmux pane
  - app-child daemon starts and listens on the custom socket
  - app-child daemon still never reaches a non-empty `ui.bootstrap.v2` in that metadata-enabled lane

### Next step
- add/fix producer instrumentation or repro so the daemon shows why polling never advances past the listen state in the app-child explicit-`--tmux-socket` lane
- rerun:
  - `scripts/tests/e2e/scenarios/explicit-tmux-socket-app-child-late-server.sh`
  - any new higher-fidelity app-child repro
  - cross-repo `agtmux-term` focused metadata-enabled UI lane

## 2026-03-09 — T-XTERM-A8 opened: `shell + non-agent child` still blocks exact-row managed demotion on the fresh desktop daemon

### Fresh cross-repo evidence from `agtmux-term`
- downstream reran the normal app path with the rebuilt local binary:
  - `AGTMUX_BIN=/Users/virtualmachine/ghq/github.com/g960059/agtmux/target/debug/agtmux swift run AgtmuxTerm`
  - the app supervisor logged:
    - `restarting stale app-managed daemon ...`
    - `started managed daemon ...`
- this proves the desktop-owned socket is no longer a stale oracle
- direct fresh probe against `/Users/virtualmachine/Library/Application Support/AGTMUXDesktop/agtmuxd.sock` now shows:
  - same-session no-bleed is fixed:
    - `vm agtmux-term %2=running`
    - `vm agtmux-term %5=waiting_input`
    - `vm agtmux-term %6=waiting_input`
  - but `%6` still remains a stale managed Codex row even though tmux already says the pane is back at shell:
    - `current_cmd=zsh`
    - `presence=managed`
    - `provider=codex`
    - `activity_state=waiting_input`
- tmux process inspection of that exact row shows the remaining child process is not an agent:
  - shell pid `35774`
  - child `37202 chezmoi cd`

### Interpretation
- A7 fixed the original same-session running bleed on fresh desktop truth
- the remaining producer bug is narrower:
  - demotion currently requires “no live child process under the shell”
  - but the real contract needs “no live agent child process under the shell”
- this is still producer truth before `agtmux-term` renders it, so the next fix belongs in the daemon repo

### Next step
- add producer-side regression/E2E for `shell + non-agent child` demotion
- rerun downstream direct desktop-socket probe after the fix
- only after that reopen any term-side consumer debugging

## 2026-03-09 — T-XTERM-A7 opened: direct daemon truth still keeps stale managed Codex rows and same-session running bleed

### Fresh downstream evidence
- direct probe against the app-owned daemon socket shows producer truth is still wrong even before `agtmux-term` renders it
- current rows on the live socket include:
  - `vm agtmux-term %6` -> `presence=managed provider=codex activity=Running current_cmd=zsh`
  - same session `vm agtmux-term` -> `%2`, `%5`, `%6` all surface as `provider=codex activity=Running`
- this reproduces two user-visible symptoms as daemon truth:
  - shell demotion after `Ctrl-C` does not clear managed/provider/activity on the exact row
  - one running Codex pane can bleed `running` to sibling Codex panes in the same session

### Coverage gap
- existing coverage is not sufficient for this exact shape:
  - `managed-exit.sh` exists upstream, but it does not yet prove the live session/sibling no-bleed shape now reported
  - no upstream online E2E currently fixes the case “multiple Codex panes in one session, only one running, siblings remain idle/unmanaged”

### Next step
- add/repair producer-side tests for:
  - shell demotion on exact pane row after agent termination / `Ctrl-C`
  - same-session same-provider no-bleed with multiple Codex panes
- then rerun downstream direct bootstrap probe and the thin `agtmux-term` live canaries

## 2026-03-09 — T-SYNCV3-P2 done: Codex semantic normalization landed in the v3 daemon truth path

### Summary
- `agtmux-source-codex-jsonl` now preserves the actual Codex JSONL semantic trigger inside `SourceEventV2.payload.codex_jsonl` while leaving the old v2 `event_type` strings unchanged for compatibility.
- The preserved payload includes the inner event type (`task_complete`, `entered_review_mode`, `function_call`, etc.), turn/call identity where available, review target metadata, and the real activity timestamp.
- `agtmux-daemon-v5` now has a dedicated `codex_v3` normalizer for the frozen sync-v3 contract instead of reusing collapsed `ActivityState` semantics.

### Behavior corrections fixed in this slice
- `task_complete` now normalizes to:
  - `thread.lifecycle = idle`
  - `turn.outcome = completed`
  - no implicit `waiting_user_input`
- `entered_review_mode` now normalizes to:
  - `thread.flags.review_mode = true`
  - synthetic pending approval request entity in `pending_requests[]`
  - `thread.blocking` / `attention` derived from that request entity, not from the flag alone
- `function_call` / `custom_tool_call` now normalize to:
  - `thread.execution = tool_running`
  - no flattening back to generic running semantics in the v3 path
- `exited_review_mode` resolves the synthetic approval request and clears derived blocking state

### Implementation notes
- Added `crates/agtmux-daemon-v5/src/codex_v3.rs` as a standalone daemon normalizer module.
- Extended `SyncV3Reducer` with the minimal helpers needed for this slice:
  - request resolution by predicate
  - `review_mode` flag mutation
  - Codex `provider_raw` update
- Kept v2 projection behavior untouched; this slice does not wire live `ui.bootstrap.v3` / `ui.changes.v3` yet.

### Gate
- `cargo test -p agtmux-source-codex-jsonl -p agtmux-daemon-v5` PASS

## 2026-03-09 — T-SYNCV3-P2-CLAUDE done: Claude field-group authority merge landed in the v3 daemon truth path

### Summary
- `agtmux-source-claude-hooks` now preserves hook-native identity inside `SourceEventV2.payload.claude_hook` and carries the real hook timestamp in `actual_activity_at`, so the v3 reducer can distinguish `PermissionRequest`, `Stop`, and `SubagentStop` without trusting collapsed `event_type`.
- `agtmux-source-claude-jsonl` now preserves `payload.claude_jsonl.line_type` plus timestamp/uuid/session metadata and forwards real transcript timestamps into `actual_activity_at`.
- `agtmux-daemon-v5` now has a dedicated `claude_v3` normalizer that merges Claude by field group instead of selecting a single collapsed activity winner.

### Authority split fixed in this slice
- Hooks are now authoritative for approval request truth:
  - `PermissionRequest` opens a pending approval request entity
  - `thread.blocking = waiting_approval` and `attention = approval` are derived from that request entity
- Hooks are now authoritative for explicit stop/completion truth:
  - `Stop` / `SubagentStop` resolve hook-owned pending requests
  - `thread.lifecycle = idle`
  - `turn.outcome = completed`
  - no synthetic `waiting_user_input`
- JSONL is now authoritative for execution/lifecycle hints only:
  - `tool_use` / `progress` -> `thread.execution = tool_running`
  - `tool_result` -> `thread.execution = thinking`
  - `assistant` -> `thread.lifecycle = idle`, `thread.execution = none`
  - JSONL updates do not clear hooks-derived blocking truth

### Implementation notes
- Added `crates/agtmux-daemon-v5/src/claude_v3.rs` as the Claude-side v3 normalizer.
- Extended `SyncV3Reducer` with Claude `provider_raw` merge support so hook and JSONL hints coexist in `provider_raw.claude`.
- Kept v2 projection behavior untouched; live `ui.bootstrap.v3` / `ui.changes.v3` wiring remains deferred until the full provider truth path is clean.

### Gate
- `cargo test -p agtmux-source-claude-hooks -p agtmux-source-claude-jsonl -p agtmux-daemon-v5` PASS

## 2026-03-09 — T-SYNCV3-P3-BOOTSTRAP done: additive `ui.bootstrap.v3` wired from daemon truth

### Summary
- `agtmux-runtime` now exposes `ui.bootstrap.v3` as a live additive RPC alongside the existing v2 wire.
- The handler returns frozen `version = 3` payloads built from live daemon-side sync-v3 truth, not from term-side reinterpretation of v2 activity fields.
- Codex/Claude semantic rows come from the existing sync-v3 reducer path; panes without loaded v3 semantics still get daemon-owned fallback rows instead of leaking collapsed `ActivityState` meanings into v3.

### Behavior in this slice
- `ui.bootstrap.v3` now emits:
  - strict exact identity fields
  - normalized sync-v3 pane snapshots
  - unmanaged shell rows with `session_key = shell:%pane_id`
  - managed fallback rows with `agent.lifecycle = unknown` and `thread.lifecycle = not_loaded` when the pane is managed in runtime truth but no provider semantic snapshot is loaded yet
- The live bootstrap path preserves the Phase 2 corrections:
  - Codex `task_complete` stays `idle + completed`
  - Claude approval/stop semantics stay request-truth / idle-completed
  - no re-collapse back to `waiting_input` / `waiting_approval` from v2 activity strings
- `ui.bootstrap.v3` does not touch the sync-v2 replay cursor/log and does not imply any `ui.changes.v3` support yet.

### Implementation notes
- Added a runtime-side `SyncV3LiveState` that:
  - consumes live Codex/Claude source events after gateway pull
  - keeps per-pane sync-v3 reducers for supported semantic providers
  - overlays strict tmux identity at bootstrap build time
  - drops managed rows when exact tmux identity cannot be resolved
- Poll loop now stamps any pane-targeted event with generation/birth identity from the live tracker when the source omitted it, so v3 rows stay exact-row stable.

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux` PASS

### Intentional deferrals
- `ui.changes.v3` remains unimplemented
- bootstrap freshness currently uses row-age summary rather than separate blocking/execution freshness clocks

## 2026-03-09 — T-SYNCV3-P3-CHANGES done: additive `ui.changes.v3` wired from daemon truth

### Summary
- `agtmux-runtime` now exposes `ui.changes.v3` as the additive follow-on to `ui.bootstrap.v3`, driven from the same daemon-owned sync-v3 row truth rather than any term-side reinterpretation.
- The v3 runtime path now keeps a canonical row store, replay cursor, and incremental change log keyed by the frozen exact identity contract.
- Upserts carry the full normalized pane snapshot plus explicit `field_groups`; removals carry strict top-level identity only and never drop `pane_instance_id`.

### Behavior in this slice
- `ui.bootstrap.v3` now returns a `replay_cursor` so consumers can bootstrap and immediately continue with `ui.changes.v3`.
- `ui.changes.v3` now emits:
  - strict top-level identity on every change entry
  - `kind = upsert` with the full sync-v3 pane row
  - `kind = remove` with no nested pane payload
  - field-group diffs for identity/presence/provider/agent/thread/pending_requests/attention/freshness/provider_raw
- The live change feed preserves the earlier semantic corrections:
  - Codex `task_complete` stays `thread.lifecycle = idle` + `turn.outcome = completed`
  - Codex/Claude tool execution stays `execution = tool_running`
  - Claude approval truth stays request-driven (`pending_requests[].request_id`, `blocking = waiting_approval`, attention summary)
  - no synthetic collapse back into v2 `waiting_input` / `waiting_approval`

### Implementation notes
- Extended `SyncV3LiveState` to:
  - reconcile canonical rows from provider reducers + managed fallback rows + unmanaged inventory rows
  - append incremental v3 upsert/remove changes whenever those canonical rows change
  - expose a replay cursor for bootstrap and replay batches for `ui.changes.v3`
- Poll loop now reconciles the sync-v3 row store on each tick after daemon truth updates and shell demotion, so the v3 feed preserves intermediate daemon-side truth transitions instead of collapsing them to the next client read.
- Server wiring now:
  - reconciles sync-v3 state before serving `ui.bootstrap.v3` / `ui.changes.v3`
  - returns `resync_required` for invalid/ahead-of-head v3 cursors
  - keeps sync-v2 replay handlers untouched

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux` PASS

### Intentional deferrals
- v3 replay trimming / epoch continuity hardening is still deferred; this slice keeps an in-memory untrimmed v3 log
- freshness still uses the current row-age summary rather than separate blocking/execution freshness clocks

## 2026-03-09 — T-SYNCV3-CLEANUP-COMPAT done: legacy activity collapse made explicitly sync-v2-only

### Summary
- Cleaned up the remaining daemon-side `ActivityState` / `activity.*` collapse helpers so they read as sync-v2 compatibility plumbing rather than generic daemon truth.
- The old projection parser and poller event encoder now explicitly advertise that they serve only the legacy sync-v2 projection / replay boundary.
- Added a guardrail test proving the sync-v3 runtime path still honors provider-native Codex payload truth even when the legacy `event_type` string is deliberately contradictory.

### Implementation notes
- Renamed the projection helper to `parse_sync_v2_compat_activity_state()` and tightened its documentation around:
  - legacy `activity.*` / `lifecycle.*` / `thread.*` / `turn.*` compatibility
  - non-applicability to sync-v3 truth reducers
- Renamed the poller helper to `sync_v2_compat_activity_event_type()` and documented that the collapsed `event_type` namespace is for the old boundary only.
- Added a runtime test that mutates a Codex `task_complete` event to carry `event_type = "activity.running"` and still expects sync-v3 bootstrap to emit `thread.lifecycle = idle` + `turn.outcome = completed`.

### Gate
- `cargo test -p agtmux-daemon-v5 sync_v2_compat_activity_state_parsing -- --nocapture` PASS
- `cargo test -p agtmux-source-poller sync_v2_compat_activity_state_mapping_to_event_type -- --nocapture` PASS
- `cargo test -p agtmux sync_v3_runtime::tests::build_bootstrap_ignores_legacy_event_type_when_codex_payload_has_v3_truth -- --nocapture` PASS

### Intentional deferrals
- sync-v2 transport / replay / CLI-facing activity fields still remain for compatibility
- broader v2 deletion is still deferred; this slice only isolates the legacy collapse more clearly

## 2026-03-09 — T-SYNCV3-CLEANUP-PAYLOAD-TESTS done: sync-v3 tests now default to payload/native truth

### Summary
- Cleaned up sync-v3 reducer/runtime tests so native payload semantics are now the default source of truth in fixtures, while legacy `activity.*` strings are treated as compat-only overrides.
- Codex and Claude v3 reducer tests no longer need semantic `event_type` strings for their main cases; they now carry neutral compat strings unless a test explicitly wants to prove the override is ignored.
- Runtime-side Codex fixtures used by sync-v3 bootstrap tests now also default to neutral compat strings, and the remaining direct Codex JSONL pre-ingest tests in `poll_loop` now carry real `payload.codex_jsonl` semantics instead of empty payloads.

### Implementation notes
- `codex_v3.rs`
  - split the test builder into a payload-first default helper plus a compat override helper
  - added a focused test showing `task_complete` still normalizes to idle+completed even if `event_type = activity.running`
- `claude_v3.rs`
  - changed hook/JSONL test builders to default to neutral compat strings
  - added focused tests showing `PermissionRequest` and JSONL `tool_use` still drive blocking/execution from native payload truth even with contradictory compat strings
- `sync_v3_runtime.rs` / `server.rs`
  - Codex v3 fixture builders now default to neutral compat strings
- `poll_loop.rs`
  - direct deterministic Codex JSONL pre-ingest tests now include `payload.codex_jsonl.inner_type = task_started` instead of empty payloads

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux-daemon-v5 task_complete_ignores_contradictory_compat_event_type_when_payload_truth_exists -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 permission_request_ignores_contradictory_compat_event_type_when_hook_payload_exists -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 jsonl_tool_use_ignores_contradictory_compat_event_type_when_payload_exists -- --nocapture` PASS
- `cargo test -p agtmux sync_v3_runtime::tests::build_bootstrap_ignores_legacy_event_type_when_codex_payload_has_v3_truth -- --nocapture` PASS
- `cargo test -p agtmux poll_tick_pulls_from_codex_jsonl_source -- --nocapture` PASS

### Intentional deferrals
- sync-v2 transport / replay / compat `event_type` strings still remain for old consumers
- this slice does not delete any compat transport or change runtime wire semantics

## 2026-03-09 — T-SYNCV3-CLEANUP-RUNTIME-V2-WIRE done: sync-v2 runtime builders extracted behind a compat module

### Summary
- Moved the remaining `ui.bootstrap.v2` / `ui.changes.v2` payload builders out of `crates/agtmux-runtime/src/server.rs` into a dedicated compat-only runtime module.
- The server keeps the public JSON-RPC methods unchanged, but the runtime wire layer now makes it much clearer that v2 bootstrap/changes are legacy compatibility surfaces rather than the main product path.
- Added focused handler tests proving sync-v2 ack/compaction still works while the sync-v3 replay cursor remains untouched.

### Implementation notes
- Added `crates/agtmux-runtime/src/sync_v2_compat.rs` containing:
  - `parse_replay_cursor()`
  - `build_ui_bootstrap_v2()`
  - `build_ui_changes_v2()`
  - the sync-v2-only DTO assembly helpers (`build_sync_v2_pane_list`, resync payload, change entry mapping)
- `crates/agtmux-runtime/src/server.rs` now imports those compat helpers instead of carrying the v2 builder implementation inline.
- `crates/agtmux-runtime/src/main.rs` now wires the new compat module explicitly.
- Added a shared server test helper that populates the sync-v2 replay log, then used it to lock:
  - `ui.bootstrap.v2` compacts sync-v2 replay without touching the sync-v3 cursor
  - `ui.changes.v2` acknowledges/compacts sync-v2 replay without touching the sync-v3 cursor
  - `ui.bootstrap.v3` still does not compact sync-v2 replay

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux ui_bootstrap_v2_handler_compacts_sync_v2_without_touching_sync_v3_cursor -- --nocapture` PASS
- `cargo test -p agtmux ui_changes_v2_handler_acknowledges_sync_v2_without_touching_sync_v3_cursor -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_handler_does_not_compact_sync_v2_log -- --nocapture` PASS

### Intentional deferrals
- sync-v2 transport / replay deletion is still deferred
- no `ui.bootstrap.v3` / `ui.changes.v3` semantic changes were made in this slice

## 2026-03-09 — T-SYNCV3-CLEANUP-SOURCE-V2-EVENT-TYPES done: source-side legacy `event_type` mapping extracted behind shared compat helpers

### Summary
- Moved the remaining source-side legacy `event_type` string tables behind a shared `agtmux-core-v5::sync_v2_compat` module so source code reads as provider payload/native truth first and explicit sync-v2 compatibility second.
- Poller, Claude hooks, Claude JSONL, and the touched Codex JSONL translator/heartbeat paths still emit the exact same strings as before, but the compat mapping now lives in one place instead of being redefined per source.
- This keeps sync-v3 truth cleanup moving in the same direction as the runtime v2 wire extraction: v3/native semantics stay in payloads, while collapsed sync-v2 compatibility strings are isolated plumbing.

### Implementation notes
- Added `crates/agtmux-core-v5/src/sync_v2_compat.rs` with:
  - `activity_event_type(ActivityState)`
  - `claude_hook_event_type(hook_type)`
  - `claude_notification_event_type(notification_type)`
  - `claude_jsonl_event_type(line_type)`
  - focused helper tests that lock the exact legacy strings
- `crates/agtmux-source-poller/src/source.rs`
  - removed the local `sync_v2_compat_activity_event_type()` table and now imports the shared helper directly
- `crates/agtmux-source-claude-hooks/src/translate.rs`
  - `normalize_event_type()` / `resolve_event_type()` now delegate compat string generation to the shared helper
- `crates/agtmux-source-claude-jsonl/src/translate.rs`
  - the JSONL line-type compat mapping now comes from the shared helper instead of an inline match
- `crates/agtmux-source-codex-jsonl/src/translate.rs`
  - running / waiting compat strings and idle heartbeat now use the shared activity helper
- `crates/agtmux-source-claude-jsonl/src/source.rs`
  - idle bootstrap / ambiguous bootstrap / heartbeat events now use the shared compat helper explicitly

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux-core-v5 sync_v2_compat -- --nocapture` PASS
- `cargo test -p agtmux-source-poller sync_v2_compat_activity_state_mapping_to_event_type -- --nocapture` PASS
- `cargo test -p agtmux-source-claude-hooks event_type_normalization -- --nocapture` PASS
- `cargo test -p agtmux-source-claude-jsonl translate::tests -- --nocapture` PASS
- `cargo test -p agtmux-source-codex-jsonl translate::tests -- --nocapture` PASS

### Intentional deferrals
- sync-v2 transport / replay deletion is still deferred
- no source semantic rewrite or sync-v3 reducer behavior change was made in this slice

## 2026-03-09 — T-SYNCV3-CLEANUP-PROJECTION-V2-PARSER done: projection compat parser moved into shared core helper

### Summary
- Moved the remaining legacy `event_type -> ActivityState` parser out of `crates/agtmux-daemon-v5/src/projection.rs` and into `agtmux-core-v5::sync_v2_compat` so both sync-v2 compat encoding and decoding now live behind the same shared core boundary.
- Projection now consumes the shared parser instead of owning a local duplicate, but the supported alias set is unchanged: `activity.*`, `lifecycle.*`, `thread.*`, and `turn.*` still collapse exactly the same way for sync-v2 projection/replay compatibility.
- This is cleanup only. Sync-v3 reducers still ignore these compat strings in favor of provider-native payload truth.

### Implementation notes
- `crates/agtmux-core-v5/src/sync_v2_compat.rs`
  - added `parse_activity_state(event_type)` covering the full legacy parser surface that projection previously owned
  - added focused core tests locking the parse behavior for:
    - `activity.*` / `lifecycle.*`
    - Claude JSONL compat aliases (`activity.user_input`, `activity.tool_complete`)
    - old Codex app-server aliases (`thread.*`, `turn.*`)
- `crates/agtmux-daemon-v5/src/projection.rs`
  - imports `agtmux_core_v5::sync_v2_compat`
  - removed the local `parse_sync_v2_compat_activity_state()` implementation
  - existing daemon tests now exercise the shared parser directly, while projection behavior tests continue to prove the parser is still consumed on real code paths

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux-core-v5 sync_v2_compat -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 sync_v2_compat_activity_state_parsing -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 heartbeat_on_new_pane_sets_initial_activity -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 real_stop_event_correctly_sets_idle -- --nocapture` PASS

### Intentional deferrals
- sync-v2 transport / replay deletion remains deferred
- no sync-v3 semantic path changed in this slice

## 2026-03-09 — T-SYNCV3-CODEX-WAITING-INPUT-PROOF done: Codex `task_complete` mismatch is intentional producer contract divergence

### Summary
- Investigated the live T-119 blocker where the same interactive Codex pane stayed `waiting_input` in `agtmux json` while `ui.bootstrap.v3` surfaced the row as `completed_idle`.
- The mismatch is by design in producer code, not consumer reinterpretation:
  - the legacy sync-v2 projection/json path still consumes Codex source `event_type = activity.waiting_input`
  - the sync-v3 reducer intentionally normalizes the same `payload.codex_jsonl.inner_type = task_complete` event to `thread.lifecycle = idle`, `thread.blocking = none`, `turn.outcome = completed`
- This follows the frozen v3 contract: `task_complete` must not automatically become `waiting_user_input` unless there is an explicit unresolved input request entity.

### Source proof
- `crates/agtmux-source-codex-jsonl/src/fsm.rs`
  - `task_complete` still transitions the legacy source FSM to `WaitingInput`
- `crates/agtmux-source-codex-jsonl/src/source.rs`
  - Codex JSONL source still emits a non-heartbeat `activity.waiting_input` compat event for `task_complete`
- `crates/agtmux-daemon-v5/src/codex_v3.rs`
  - `task_complete` explicitly calls `finish_turn(..., ThreadLifecycleV3::Idle, TurnOutcomeV3::Completed)` and does not create a user-input request
- `/tmp/agtmux-status-v3-final-design-20260309.md`
  - explicitly says `task_complete` does not automatically imply `waiting_user_input`

### Implementation notes
- Added a focused runtime proof test in `crates/agtmux-runtime/src/server.rs` that feeds the exact same Codex `task_complete` source event into:
  - the sync-v2/list/json path, which stays `presence=managed provider=codex activity_state=WaitingInput current_cmd=node`
  - the sync-v3 bootstrap path, which returns the same pane identity as `presence=managed provider=codex thread.lifecycle=idle turn.outcome=completed`
- No producer semantics changed in this slice; the test and docs make the intended divergence explicit for downstream consumers.

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux codex_task_complete_intentionally_diverges_between_sync_v2_and_v3_surfaces -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_emits_strict_identity_and_normalized_codex_truth -- --nocapture` PASS
- `cargo test -p agtmux-daemon-v5 task_complete_normalizes_to_idle_completion_without_blocking -- --nocapture` PASS
- `cargo test -p agtmux-source-codex-jsonl poll_files_emits_waiting_input_on_task_complete -- --nocapture` PASS

### Consumer implication
- term cannot rely on `waiting_user_input` for Codex rows in sync-v3 unless producer truth includes an explicit pending input request
- `completed_idle` for Codex after `task_complete` is the canonical v3 truth, even when sync-v2/json still shows `waiting_input`

## 2026-03-09 — T-XTERM-A6b done: app-child exact-socket managed promotion no longer depends on PATH finding `ps` / `lsof`

### Summary
- Investigated the new upstream blocker where the term-side targeted XCUITest could directly probe the pane on the exact tmux socket, but `ui.bootstrap.v3` still surfaced only one unmanaged row such as `presence=unmanaged provider=nil session_key=shell:%0 current_cmd=zsh freshness=down`.
- The failure was upstream of sync-v3 row composition. `ui.bootstrap.v3` only emits that unmanaged fallback row when:
  - tmux inventory already contains the pane in `last_panes`
  - but neither the daemon managed projection nor the sync-v3 reducer has any truth for that pane
- For a plain shell pane hosting a live Codex child, producer promotion depends on metadata tools:
  - `scan_all_processes()` must succeed so deep process inspection can turn `current_cmd=zsh` into `process_hint=codex`
  - Codex / Claude discovery may also call `lsof`
- `tmux` already had hardened binary resolution via `TMUX_BIN` + standard-path fallback, but `ps` and `lsof` still used bare PATH lookup. In app-child / XCUITest environments that can leave inventory working on the exact socket while the metadata path fails closed, which exactly matches the observed `shell:%0` bootstrap row.

### Implementation notes
- Added `agtmux-core-v5::system_bin` as the shared producer-side system binary resolver.
  - `resolve_ps_bin()` falls back to `/bin/ps`, `/usr/bin/ps`
  - `resolve_lsof_bin()` falls back to `/usr/sbin/lsof`, `/usr/bin/lsof`
- Updated `crates/agtmux-tmux-v5/src/capture.rs`
  - `scan_all_processes()` now resolves `ps` through the shared helper before spawning
  - failure logging now includes the resolved spawn target for easier future diagnosis
- Updated `crates/agtmux-source-codex-jsonl/src/discovery.rs`
  - `get_cwd_via_lsof()` now resolves `lsof` through the shared helper
- Updated `crates/agtmux-source-claude-jsonl/src/discovery.rs`
  - fd-based JSONL discovery now resolves `lsof` through the same helper

### Producer implication
- This slice does not change sync-v3 truth semantics or identity rules.
- It fixes the upstream condition where managed truth never formed for the pane, so bootstrap can again surface the correct managed Codex row on the exact socket instead of falling back to unmanaged shell inventory.

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux-core-v5 system_bin -- --nocapture` PASS
- `cargo test -p agtmux-tmux-v5 snapshot_deep_inspection_shell_descendant_codex -- --nocapture` PASS
- `cargo test -p agtmux-source-codex-jsonl get_cwd_via_lsof_invalid_pid_returns_none -- --nocapture` PASS
- `cargo test -p agtmux-source-claude-jsonl discover_jsonl_via_lsof_nonexistent_pid_returns_none -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS

## 2026-03-09 — T-XTERM-A6c done: direct socket visibility and pre-launch bootstrap agree on unmanaged shell truth

### Summary
- Re-read the downstream targeted UI test and found the failing `waitForAppDaemonBootstrapReady(...)` gate runs before the test sends the live `codex exec` command.
- At that point, the downstream app has only proved two things:
  - the exact tmux socket contains the target pane
  - the pane's current command is still plain `zsh`
- That is inventory truth, not provider truth. Producer bootstrap is therefore expected to return the same pane as unmanaged `shell:%pane` until an actual Codex source event arrives.

### Source proof
- `agtmux-term/Tests/AgtmuxTermUITests/AgtmuxTermUITests.swift`
  - `waitForAppDaemonBootstrapReady(...)` is called before the test sends `codex exec ...`
  - the helper still waits for `target?.presence == "managed"` and `target?.provider != nil`, which is stronger than current producer semantics for a pre-launch plain shell pane
- `crates/agtmux-runtime/src/server.rs`
  - `build_ui_bootstrap_v3()` only surfaces managed rows from daemon projection or sync-v3 reducers
  - otherwise it emits the same pane identity via unmanaged fallback (`session_key = shell:%pane`)
- `crates/agtmux-runtime/src/sync_v3_runtime.rs`
  - `compose_rows()` picks:
    - reducer snapshot when provider truth exists
    - managed fallback only when daemon projection already has managed truth
    - unmanaged shell inventory row otherwise

### Implementation notes
- Added a focused runtime proof test that uses the same pane identity in both views:
  - cached inventory row: `%0`, session `agtmux-e2e-managed`, `current_cmd=zsh`, `presence=unmanaged`
  - `ui.bootstrap.v3` row before provider truth: same `%0`, same session/window identity, `session_key=shell:%0`, `presence=unmanaged`, `provider=nil`
  - same `ui.bootstrap.v3` row after a Codex source event: `presence=managed`, `provider=codex`, `session_key=codex:%0`

### Consumer implication
- `appDirectSocketProbe` proving `tmux -S <socket> list-panes` can see `%0 zsh` does not imply producer-managed bootstrap truth yet.
- term cannot require `presence=managed provider!=nil` before it has actually launched Codex or otherwise caused provider truth to arrive for that pane.

### Gate
- `cargo fmt --all` PASS
- `cargo test -p agtmux plain_shell_inventory_remains_unmanaged_in_bootstrap_until_provider_truth_arrives -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_emits_strict_identity_and_normalized_codex_truth -- --nocapture` PASS

## 2026-03-09 — T-XTERM-A6d done: repo-owned exact-socket Codex proof shows managed truth mid-flight

### Summary
- To get out of the flaky XCUITest harness loop, added a repo-owned explicit-socket Codex proof scenario that runs a deliberately long `codex exec` task under the same app-like daemon env shape (`env -i`, explicit `--tmux-socket`, normalized PATH/HOME/USER/etc.).
- The scenario captures the same pane across three surfaces during the live run:
  - tmux exact-socket row: `session|window|pane|pid|current_command`
  - daemon `list_panes_snapshot`
  - daemon `ui.bootstrap.v3`
- On this machine the proof is green:
  - mid-flight at 8 seconds, both daemon surfaces showed `presence=managed provider=codex` for the exact pane
  - immediate post-completion snapshot still showed managed Codex completion truth
    - sync-v2 compat row: `activity_state=WaitingInput`
    - sync-v3 row: `agent.lifecycle=completed`, `thread.lifecycle=idle`, `turn.outcome=completed`

### Concrete observed proof
- Mid-flight snapshot (`PROVIDER=codex bash scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh`)
  - tmux exact-socket row:
    - `e2e-explicit-codex-midflight-<pid>|@0|%0|<pane_pid>|node`
  - `list_panes_snapshot` target row:
    - `presence=managed`
    - `provider=codex`
    - `current_cmd=node`
    - `evidence_mode=deterministic`
  - `ui.bootstrap.v3` target row:
    - `presence=managed`
    - `provider=codex`
    - `thread.lifecycle=active`
    - `thread.execution=tool_running`
- Immediate post-completion snapshot:
  - tmux exact-socket row had already returned to `...|zsh`
  - daemon `list_panes_snapshot` still held managed Codex completion truth for that tick
  - `ui.bootstrap.v3` still held managed completion truth for that tick

### Implementation notes
- Added `daemon_rpc()` to `scripts/tests/e2e/harness/common.sh`
  - shell scenarios can now query daemon JSON-RPC endpoints directly, including `list_panes_snapshot` and `ui.bootstrap.v3`
- Added `scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh`
  - codex-specific exact-socket proof scenario
  - writes mid-flight and final probe triplets under its temp workdir for repro/debug
- Added the new scenario to `scripts/tests/e2e/online/run-all.sh` for `PROVIDER=codex`

### Consumer implication
- The remaining term-side red after provider launch was not reproduced as an upstream producer miss in the repo-owned exact-socket lane.
- Producer truth exists mid-flight for the exact pane.
- A final unmanaged shell snapshot in term can still be a later observation after the producer has already surfaced managed/run or managed/completed truth earlier.

### Gate
- `bash -n scripts/tests/e2e/harness/common.sh` PASS
- `bash -n scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh` PASS
- `bash -n scripts/tests/e2e/online/run-all.sh` PASS
- `PROVIDER=codex bash scripts/tests/e2e/scenarios/explicit-tmux-socket-codex-midflight-proof.sh` PASS

## 2026-03-09 — T-XTERM-A6e done: Codex node-runtime discovery no longer needs a direct process hint

### Contradiction analysis
- The fresh term contradiction against `f559187` was real enough to require an upstream runtime comparison, not consumer reinterpretation:
  - repo-owned proof: exact-socket daemon launched from the shell, mid-flight pane truth becomes `managed provider=codex`
  - term T-149: exact-socket wait gate observes `pane_current_command=node|codex` before bootstrap polling, but `ui.bootstrap.v3` stays `unmanaged provider=nil` for 45 seconds
- Reading the runtime path showed a remaining producer-side dependency that the shell proof did not need:
  - Step 6a (`poll_loop.rs`) only attempted Codex JSONL discovery when `process_hint == "codex"`
  - the same step also skipped entirely when `metadata_failure_reason` was set, even though tmux current_path + Codex session files do not depend on deep process inspection success
  - `agtmux-source-codex-jsonl` still resolved the sessions root from `HOME` only, instead of preferring `CODEX_HOME`

### Why this fits the term contradiction
- In the term lane, the exact socket already reported `pane_current_command=node|codex`, which means the same pane could legitimately sit on a neutral `node` runtime while Codex is live.
- If that pane does not surface a direct `process_hint=codex` at the same tick, the previous daemon code would not even attempt Codex JSONL discovery for it.
- That makes the app-child lane materially different from the shell proof path:
  - shell proof can still win from direct process truth
  - app-child lane can need tmux current_path + CODEX_HOME + JSONL bootstrap truth to promote the same pane

### Implementation notes
- `crates/agtmux-runtime/src/poll_loop.rs`
  - added an explicit Codex candidate helper that includes neutral `node` runtimes while still excluding shell / Claude / unknown hints
  - removed the coarse `metadata_failure_reason` gate from Step 6a so Codex JSONL discovery can still run when deep process inspection is degraded
  - added a focused runtime regression test proving that a `node` pane with no direct process hint is promoted from a real Codex session file
- `crates/agtmux-source-codex-jsonl/src/discovery.rs`
  - session-root resolution now prefers `CODEX_HOME` before falling back to `HOME/.codex`
  - added pure tests for `CODEX_HOME` vs `HOME` resolution

### Consumer implication
- The earlier exact-socket proof and the term contradiction were not mutually exclusive: the producer still had a node-runtime-specific gap in managed-truth formation.
- After this fix, Codex exact-socket managed promotion no longer depends on getting a direct `process_hint=codex` first.

### Gate
- `cargo test -p agtmux poll_tick_discovers_codex_jsonl_from_node_runtime_without_process_hint -- --nocapture` PASS
- `cargo test -p agtmux codex_jsonl_candidates_include_neutral_node_runtime -- --nocapture` PASS
- `cargo test -p agtmux-source-codex-jsonl codex_home_dir_ -- --nocapture` PASS

## 2026-03-09 — T-XTERM-A6f done: sync-v3 preserves linked-session exact rows and shell→managed promotion identity is now explicitly documented

### Investigation result
- The strongest remaining H1 seam was real in producer code: `SyncV3LiveState` still stored live rows in a `BTreeMap` keyed only by `pane_id`.
- That meant tmux inventory could contain multiple exact locations for the same live pane (`session_name` / `window_id` differ in linked-session topologies), but sync-v3 bootstrap/changes would silently keep only one surviving row.
- This was separate from the already-proven shell→managed promotion pattern:
  - same visible pane location
  - same `pane_instance_id`
  - `session_key` changes from `shell:%pane` to `<provider>:%pane` once provider truth arrives
- Term-side conflict/drop findings fit that producer shape: the daemon really can send later managed upserts whose identity differs from the earlier shell bootstrap row only in the strict provider/session identity fields.

### Implementation notes
- `crates/agtmux-runtime/src/sync_v3_runtime.rs`
  - live reducer ownership stays keyed by `pane_id`
  - emitted sync-v3 rows are now keyed internally by the exact location tuple `(session_name, window_id, pane_id)` so linked-session rows no longer collapse
  - reconcile/remove logic now works per exact location row rather than per bare `pane_id`
- `crates/agtmux-runtime/src/server.rs`
  - added focused proofs that:
    - sync-v2 compat cache/list snapshot still compacts a linked managed pane by `pane_id`
    - `ui.bootstrap.v3` now returns both linked exact rows for the same managed pane
    - `ui.changes.v3` promotes a shell row to managed at the same visible location with stable `pane_instance_id` but changed `session_key`
- `crates/agtmux-runtime/src/poll_loop.rs`
  - widened Codex Step 6a candidate selection so `process_hint=runtime_unknown` with `current_cmd=node` is treated the same as other neutral node-runtime Codex discovery candidates
- `docs/80_decisions/ADR-20260309-sync-v3-contract-freeze.md`
  - clarified that `pane_id` alone is not a unique v3 row key in linked-session topologies
  - documented that shell→managed promotion may legitimately change `session_key` while keeping `pane_instance_id` stable

### Consumer implication
- Upstream daemon truth now preserves linked-session exact row identity in sync-v3 itself; the old `pane_id` collapse is no longer a valid explanation for missing linked rows in bootstrap/changes.
- The remaining shell→managed promotion pattern is still by design:
  - same visible location
  - same `pane_instance_id`
  - different `session_key`
- A strict term consumer must therefore accept managed upserts that replace a shell row at the same visible location instead of silently dropping them as impossible conflicts.

### Gate
- `cargo test -p agtmux build_bootstrap_preserves_linked_session_locations_for_same_pane_id -- --nocapture` PASS
- `cargo test -p agtmux reconcile_removes_only_missing_linked_session_location -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_preserves_linked_session_rows_even_when_v2_cache_compacts -- --nocapture` PASS
- `cargo test -p agtmux ui_changes_v3_promotes_same_visible_row_with_stable_pane_instance_id -- --nocapture` PASS
- `cargo test -p agtmux codex_jsonl_candidates_include_neutral_node_runtime -- --nocapture` PASS

## 2026-03-09 — T-XTERM-A6g done: `ui.changes.v3` now replaces exact-identity churn instead of mutating it in place

### Investigation result
- The new cross-repo repro exposed a real daemon-side contract bug in the changes lane:
  - bootstrap for a plain shell row was valid (`session_key = shell:%pane`, unmanaged, same exact visible location)
  - later provider truth for the same visible pane could arrive with:
    - same `session_name`
    - same `window_id`
    - same `pane_id`
    - same `pane_instance_id`
    - different `session_key` (`codex:%pane` / `claude:%pane`)
  - the runtime previously emitted that as a single `upsert` with `field_groups` containing `identity`
- For a strict consumer keyed by exact identity, that is ambiguous: the old exact row was never explicitly removed, so a conflict-drop path was plausible downstream.
- `freshness.down` was not the provider signal here. Freshness remains orthogonal summary state; the actual producer disagreement was the in-place exact-identity mutation.

### Implementation notes
- `crates/agtmux-runtime/src/sync_v3_runtime.rs`
  - reconcile now treats any same-location exact-identity delta as row replacement:
    - `remove(old exact identity)`
    - `upsert(new exact identity)` with a full field-group payload
  - non-identity deltas still use the existing structured field-group upsert path
- `crates/agtmux-runtime/src/server.rs`
  - updated the shell→managed promotion proof so bootstrap still yields the unmanaged shell row, while `ui.changes.v3` now returns two ordered changes:
    - remove the shell exact row
    - upsert the managed exact row
- `docs/80_decisions/ADR-20260309-sync-v3-contract-freeze.md`
  - now explicitly states that same-location exact-identity changes must be represented as remove+upsert, not as an in-place conflicting upsert

### Consumer implication
- The daemon was at fault for this specific promotion shape in the changes lane.
- After this fix:
  - linked-session exact rows are still preserved
  - same-location shell→managed promotion keeps `pane_instance_id` stable
  - `session_key` may still change by design
  - but the changes feed now expresses that as an explicit row replacement that strict consumers can apply without conflict heuristics

### Gate
- `cargo test -p agtmux build_changes_replaces_row_when_exact_identity_changes_at_same_location -- --nocapture` PASS
- `cargo test -p agtmux build_changes_replaces_row_when_claude_promotion_changes_exact_identity -- --nocapture` PASS
- `cargo test -p agtmux ui_changes_v3_replaces_shell_row_when_exact_identity_changes_on_promotion -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_emits_unmanaged_row_when_no_semantic_truth_exists -- --nocapture` PASS
- `cargo test -p agtmux ui_changes_v3_emits_upsert_with_strict_identity_from_sync_v3_truth -- --nocapture` PASS

## 2026-03-10 — T-SYNCV3-FRESHNESS-FALLBACK done: managed fallback rows now age freshness from `updated_at`

### Investigation result
- The widespread provider-adjacent `freshness.down` symptom split into two separate causes:
  - Cause 1: a real daemon bug in the managed fallback constructor
  - Cause 2: the existing row-age policy for reducer-backed rows after `>15s`
- Cause 1 was the immediate product bug:
  - `crates/agtmux-runtime/src/sync_v3_runtime.rs`
  - `compose_rows()` selected `build_managed_fallback_snapshot()` whenever the daemon projection already had provider truth for a pane but the sync-v3 reducer had not loaded native semantics yet
  - that constructor hard-coded `freshness = down/down/down`
  - so provider-attributed fallback rows (`presence=managed`, `provider=codex|claude`, `thread.lifecycle=not_loaded`) were born `freshness.down` even when `managed.updated_at` was only seconds old
- Cause 2 still exists but was not changed in this slice:
  - reducer-backed rows already use `freshness_from_updated_at(updated_at, now)`
  - with the current fixed thresholds, any row that stays idle/waiting for `>15s` ages to `freshness.down`
  - that policy may still be user-hostile product-wise, but it is not the constructor bug that caused fallback rows to show `down` immediately

### Implementation notes
- `crates/agtmux-runtime/src/sync_v3_runtime.rs`
  - `build_managed_fallback_snapshot()` now derives freshness from `managed.updated_at` via `freshness_from_updated_at(...)`
  - `thread.lifecycle = not_loaded` remains unchanged, so semantic incompleteness is still represented explicitly without conflating it with a freshness outage
- `crates/agtmux-runtime/src/server.rs`
  - added a focused `ui.bootstrap.v3` regression proving that a provider-attributed fallback row is surfaced as:
    - `presence = managed`
    - `provider = codex`
    - `thread.lifecycle = not_loaded`
    - `freshness.snapshot = fresh`

### Gate
- `cargo test -p agtmux sync_v3_runtime::tests::managed_fallback_does_not_reuse_collapsed_v2_activity_state -- --nocapture` PASS
- `cargo test -p agtmux sync_v3_runtime::tests::managed_fallback_freshness_tracks_projection_updated_at -- --nocapture` PASS
- `cargo test -p agtmux ui_bootstrap_v3_managed_fallback_ages_freshness_from_projection_updated_at -- --nocapture` PASS

### Intentional deferral
- Cause 2 remains open: reducer-backed idle / waiting rows still age to `down` after `>15s` under the current row-age summary policy
- live regression pin for this Cause 1 fix stays downstream in the existing `agtmux-term` managed-provider live proof rather than a new daemon-owned online lane
