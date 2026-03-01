# Progress Ledger (append-only)

## Rules
- Append only. 既存履歴は書き換えない。
- 記録対象: 仕様変更、判断、ユーザー要望、学び、gate証跡。

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
