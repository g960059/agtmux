# Design (mutable; changes common)

## How to read this doc
- `Main (MVP Slice)` is the implementation blocker for Phase 1-2.
- `Appendix (Post-MVP Hardening)` is intentionally non-blocking during Phase 1-2.
- If implementation hits an Appendix dependency, promote it via `docs/60_tasks.md` and `docs/70_progress.md`.

## Main (MVP Slice)

### 1) Interfaces / APIs (MVP)

#### Source Server -> Gateway
- Transport: UDS JSON-RPC (newline-delimited JSON)
- Method: `source.pull_events`
- Request:
```json
{"id":1,"method":"source.pull_events","params":{"cursor":"opaque","limit":500}}
```
- Response:
```json
{
  "id":1,
  "result":{
    "events":["SourceEventV2"],
    "next_cursor":"opaque-2",
    "heartbeat_ts":"2026-02-25T10:30:00Z",
    "source_health":{"status":"healthy|degraded|down","checked_at":"2026-..."}
  }
}
```

#### Gateway -> Daemon
- Transport: UDS JSON-RPC
- Method: `gateway.pull_events`
- Response (MVP minimal):
  - `events`
  - `next_cursor`
- Cursor policy (MVP):
  - single watermark (`committed_cursor`) only
  - daemon projection/persist 成功後に cursor を前進
- Poll interval (default): daemon 250ms / gateway 200ms

#### Daemon -> Clients
- Pull:
  - `list_panes`
  - `list_sessions`
  - `list_source_health`
- Push:
  - `state_changed`
  - `summary_changed`
- Required payload fields (`list_panes` / `state_changed`):
  - `signature_class`: `deterministic | heuristic | none`
  - `signature_reason`
  - `signature_confidence`
  - `signature_inputs` (`provider_hint`, `cmd_match`, `poller_match`, `title_match`)

#### Deterministic sources (MVP fixed)
- Codex: `agtmux-source-codex-appserver`
  - **Protocol**: 公式 Codex App Server API (JSON-RPC 2.0 over stdio)
  - **API reference**: `docs/codex-appserver-api-reference.md` (実装前に必読)
  - **Primary path**: `CodexAppServerClient` が `codex app-server` を spawn し、`thread/list` ポーリング + notification 受信
  - **Fallback path**: App Server 利用不可時のみ、tmux capture から `codex exec --json` の NDJSON を parse
  - **T-119**: `thread/list` の `cwd` パラメータで tmux pane cwd とマッチングし thread ↔ pane 対応を確立
- Claude: `agtmux-source-claude-hooks`

#### Reuse strategy (from v4)
- Reuse as crate/module:
  - poller pattern matching core
  - source health state transition logic
  - title resolution logic (canonical session index + binding history)
- Do not reuse as-is:
  - v4 orchestrator monolith
  - v4 store schema

### 2) Data Model (MVP)
```rust
pub enum EvidenceTier { Deterministic, Heuristic }
pub enum PanePresence { Managed, Unmanaged }
pub enum EvidenceMode { Deterministic, Heuristic, None }
pub enum PaneSignatureClass { Deterministic, Heuristic, None }

pub struct SourceEventV2 {
    pub event_id: String,
    pub provider: Provider,
    pub source_kind: SourceKind,
    pub tier: EvidenceTier,
    pub observed_at: DateTime<Utc>,
    pub session_key: String,
    pub pane_id: Option<String>,
    pub pane_generation: Option<u64>,
    pub pane_birth_ts: Option<DateTime<Utc>>,
    pub source_event_id: Option<String>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub confidence: f64,
}

pub struct PaneInstanceId {
    pub pane_id: String,
    pub generation: u64,
    pub birth_ts: DateTime<Utc>,
}

pub struct SessionRuntimeState {
    pub session_key: String,
    pub presence: PanePresence,
    pub evidence_mode: EvidenceMode,
    pub deterministic_last_seen: Option<DateTime<Utc>>,
    pub winner_tier: EvidenceTier,
    pub activity_state: ActivityState,
    pub activity_source: SourceKind,
    pub representative_pane_instance_id: Option<PaneInstanceId>,
    pub updated_at: DateTime<Utc>,
}

pub struct PaneRuntimeState {
    pub pane_instance_id: PaneInstanceId,
    pub presence: PanePresence,
    pub evidence_mode: EvidenceMode,
    pub signature_class: PaneSignatureClass,
    pub signature_reason: String,
    pub signature_confidence: f64,
    pub no_agent_streak: u32,
    pub updated_at: DateTime<Utc>,
}

pub struct SourceCursorState {
    pub source_kind: SourceKind,
    pub committed_cursor: Option<String>,
    pub checkpoint_ts: DateTime<Utc>,
}
```

#### Persistence schema (MVP)
- `events_raw_v2`
  - key: `(provider, source_kind, event_id)`
- `session_state_v2`
  - key: `session_key`
- `pane_state_v2`
  - key: `pane_instance_id` (`pane_id,generation,birth_ts`)
- `binding_link_v2`
  - key: `(pane_instance_id, bound_at)`
- `cursor_state_v2`
  - key: `source_kind`
- `source_health_v2`
  - key: `source_kind`

### 3) Resolver and Arbitration (MVP)
- Dedup key: `provider + session_key + event_id`
- Deterministic freshness:
  - fresh: `<= 3s`
  - stale: `> 3s`
  - down: `> 15s` or health down
- Winner selection:
  1. dedup
  2. split deterministic/heuristic
  3. deterministic fresh があれば deterministic tier を採用
  4. それ以外は heuristic tier を採用
  5. fresh deterministic 再到達で即時 re-promotion
- Source rank policy (MVP):
  - Codex: `appserver (official API) > capture fallback > poller`
  - Claude: `hooks > poller`
  - Note: Codex appserver は公式 API (`docs/codex-appserver-api-reference.md`) を使用。独自プロトコルは禁止。
- Presence rule:
  - deterministic/heuristic 切替は `presence` を変更しない
  - `presence=managed` は agent session がある限り維持

### 4) Pane Signature Classifier (v1, MVP)
- Output:
  - `signature_class`, `signature_reason`, `signature_confidence`
- Deterministic rule:
  - required fields (`provider, source_kind, pane_instance_id, session_key, source_event_id, event_time`) が揃えば `deterministic`
  - 欠ける event は `signature_inconclusive`
- Heuristic scoring:
  - process/provider hint: `1.00`
  - current_cmd token: `0.86`
  - poller capture signal: `0.78`
  - pane_title token: `0.66`
- Guardrails:
  - title-only は managed 昇格根拠にしない
  - wrapper (`node|bun|deno`) + no provider hint + title-only は reject
- Hysteresis:
  - idle確定: `max(4s, 2 * poll_interval)`
  - running昇格: running hint + `last_interaction <= 8s`
  - running降格: hint消失 + `last_interaction > 45s`
  - `no-agent` 連続2回で `none`

### 5) Binding State Machine (MVP)
- Entity:
  - key: `pane_instance_id` (`pane_id,generation,birth_ts`)
  - `session_key` は link target
- States:
  - `Unmanaged`
  - `ManagedHeuristic`
  - `ManagedDeterministicFresh`
  - `ManagedDeterministicStale`
- Key transitions:
  - heuristic signature: `Unmanaged -> ManagedHeuristic`
  - deterministic handshake: `ManagedHeuristic -> ManagedDeterministicFresh`
  - freshness超過: `ManagedDeterministicFresh -> ManagedDeterministicStale`
  - deterministic復帰: `ManagedDeterministicStale -> ManagedDeterministicFresh`
  - heuristic no-agent x2: `ManagedHeuristic -> Unmanaged`
- Pane reuse guard:
  - 同一 `pane_id` 再利用時に `generation` をインクリメント
  - grace window (`120s`) 中は tombstone を保持し誤結合を防ぐ

### 6) Pane/Session Title Resolution (MVP)
- Handshake completion:
  - deterministic event で `session_key` と `pane_instance_id` の関連が確立し、最新関連が有効
- Representative pane (session tile):
  1. 最新 deterministic handshake 時刻
  2. 同点は latest activity
  3. 同点は `pane_id` lexical order
- Title priority:
  1. canonical agent session name
  2. bound title history
  3. live pane title
  4. path-based title
  5. fallback title

### 7) Error Handling (MVP)
- Minimal taxonomy:
  - `invalid_source_event`
  - `missing_event_time`
  - `source_inadmissible`
  - `source_rank_suppressed`
  - `late_event`
  - `binding_conflict`
  - `signature_inconclusive`
  - `signature_guard_rejected`
- User-visible policy:
  - API は JSON-RPC error code + reason
  - UI は監視継続可否だけを表示（詳細は logs）

### 8) Test Strategy (MVP)
- Unit:
  - tier resolver（fresh/stale/down, re-promotion）
  - source admissibility（priority + health）
  - dedup
  - signature classifier（weights, guardrails, hysteresis）
  - binding transitions（generation + grace）
- Integration:
  - managed(heuristic) -> deterministic 昇格
  - unmanaged pane 非汚染
  - pane再利用で誤結合しない
  - source outage/recovery and suppress
- E2E/Replay:
  - deterministic drop -> fallback -> re-promotion
  - mixed provider independent health
- Accuracy gate:
  - deterministic: weighted F1 >= 0.88
  - poller fallback: weighted F1 >= 0.85 and waiting recall >= 0.85

### 9) Runtime Integration (MVP)

#### Architecture
- MVP は single-process binary (`agtmux`) で全コンポーネントを in-process 結合する。
- コンポーネント間 UDS は使わず直接関数呼び出し:
  `tmux-v5 -> poller.poll_batch() -> poller.pull_events() -> gateway.ingest_source_response() -> gateway.pull_events() -> daemon.apply_events()`
- UDS JSON-RPC server は CLI client 通信にのみ使用する。
- Multi-process extraction（supervisor + 5 child processes per C-001）は Post-MVP。
- Initial bootstrap は poller-only。Codex/Claude deterministic source adapter は health 表示用に登録するが、IO adapter 実装までイベントは生成しない。

#### tmux Integration (`agtmux-tmux-v5`)
- `TmuxCommandRunner` trait: mock injection 可能なテスト境界（v4 パターンを移植）
- `TmuxExecutor`: sync `std::process::Command` wrapper（`TmuxCommandRunner` を実装）
  - Socket targeting 優先順: `--tmux-socket` > `AGTMUX_TMUX_SOCKET_PATH` > `AGTMUX_TMUX_SOCKET_NAME` > `TMUX` env > default
- `list_panes()`: `tmux list-panes -a -F` を v4 format string で parse
  - `Vec<TmuxPaneInfo>` を返す（session_name, window_id, window_name, current_path, width, height, active 等の full metadata）
- `capture_pane()`: `tmux capture-pane -p -t {pane_id} -S -{lines}` を wrap
- `inspect_pane_processes()`: ps ベースの provider hint 抽出（claude/codex in argv）
- `PaneGenerationTracker`: `pane_id -> (generation, birth_ts)` を追跡し、pane 再利用時に generation をインクリメント
- `to_pane_snapshot()`: TmuxPaneInfo + capture + process_hint + generation -> `PaneSnapshot` に変換
- `tokio::task::spawn_blocking` 経由で呼び出し（sync subprocess）
- Error handling: `TmuxError`（`thiserror`）; 個別 capture 失敗は log + skip

#### Poll Loop
- `tokio::time::interval(Duration::from_millis(poll_ms))`（default 1000ms, `--poll-interval-ms` で設定可能）
- 毎 tick:
  1. `spawn_blocking(|| tmux.list_panes())` — 失敗時: warning log, tick skip, continue
  2. 各 pane: `spawn_blocking(|| tmux.capture_pane(pane_id, 50))` — 失敗時: pane skip
  3. 各 pane: `spawn_blocking(|| inspect_pane_processes(pane_id))`
  4. `Vec<PaneSnapshot>` を `to_pane_snapshot()` で構築（generation tracking 付き）
  5. `poller.poll_batch(&snapshots)` — agent pane はイベント生成
  6. non-agent pane（`poll_pane` が None）: synthetic "unmanaged" event を生成し daemon が全 pane を追跡（FR-009）
  7. `poller.pull_events(&request, now)` -> `PullEventsResponse`
  8. `gateway.ingest_source_response(SourceKind::Poller, response)`
  9. `gateway.pull_events(&gw_request)` -> `GatewayPullResponse`
  10. `daemon.apply_events(gw_response.events, now)` — `Vec<>` ownership move
  11. Compact: poller buffer trim, gateway committed cursor advance, daemon change log trim

#### Cursor Contract Fix（事前要件）
- 現行バグ: source は caught up 時に `next_cursor: None` を返すが、gateway は `Some` の時のみ cursor 更新する。結果、毎 tick 同じイベントが再配信される。
- Fix: source は caught up 時も現在位置を `Some(cursor)` として返す。gateway は常に tracker cursor を上書きする。

#### Memory Management (MVP)
- Poll loop は daemon apply 後に毎回 compact する:
  - Poller: 処理済みイベントを buffer から除去
  - Gateway: committed cursor を advance し buffer prefix を truncate
  - Daemon: 最終 serve 済み version より古い changes を trim
- Compaction なしでは 1s polling で pane あたり ~3.6K events/hour → 数時間で OOM。

#### UDS JSON-RPC Server
- Socket path: `/tmp/agtmux-$UID/agtmuxd.sock`（default, `--socket-path` で override 可）
- Directory: mode `0700` で作成; socket file は mode `0600`
- Stale socket detection: startup 時に connect 試行; 失敗なら remove して rebind
- Cleanup: graceful shutdown 時に socket file を remove
- Protocol: newline-delimited JSON（1行1 JSON object）, connection-per-request
- Minimal hand-rolled 実装（jsonrpc-core 依存なし）; 3 method のみ
- Methods:
  - `list_panes` -> serialized `Vec<PaneRuntimeState>`
  - `list_sessions` -> serialized `Vec<SessionRuntimeState>`
  - `list_source_health` -> serialized `Vec<(SourceKind, SourceHealthReport)>`

#### CLI Subcommands
- 全 subcommand が `--socket-path` (`-s`) を受け付ける（default `/tmp/agtmux-$UID/agtmuxd.sock`）
- `agtmux daemon` — daemon 起動（poll loop + UDS server, foreground）
  - `--poll-interval-ms`（default 1000）
  - `--tmux-socket`（tmux backend socket path）
- `agtmux status` — UDS 接続して summary 表示（pane count, agent count, source health）
- `agtmux list-panes` — UDS 接続して pane states 表示（JSON or table）
- `agtmux tmux-status` — tmux status-bar 用 single-line output

#### Signal Handling
- `tokio::signal::ctrl_c()` + SIGTERM を `tokio::select!` で処理
- Signal 受信時: poll loop 停止 → UDS listener close → socket file remove → exit 0

#### Logging
- `tracing` + `tracing-subscriber`（`AGTMUX_LOG` / `RUST_LOG` env var）
- Default: daemon mode は `info`、CLI client mode は `warn`

#### Persistence
- MVP: in-memory only。daemon restart 時は projection を scratch から再構築。
- SQLite persistence は Post-MVP。

#### Codex App Server Integration (MVP)

> **MUST READ**: `docs/codex-appserver-api-reference.md` — 公式APIリファレンス。独自プロトコルの実装は禁止。

##### Architecture: Primary (App Server) + Fallback (Capture)

```
codex app-server (stdio)              tmux capture-pane
    |                                      |
    v                                      v
CodexAppServerClient                 parse_codex_capture_events()
  - spawn + initialize handshake       - NDJSON parse
  - thread/list polling                - fingerprint dedup
  - notification → CodexRawEvent       - CodexRawEvent
    |                                      |
    +------- codex_source.ingest() --------+
                    |
                    v
              Gateway → Daemon
```

##### Primary path: CodexAppServerClient (`codex_poller.rs`)

1. **Spawn**: `codex app-server` を child process として起動 (stdin/stdout piped)
2. **Handshake**: `initialize` → response → `initialized` (10s timeout)
   - `clientInfo`: `{ name: "agtmux", title: "agtmux v5", version: "0.1.0" }`
   - 全メッセージに `"jsonrpc": "2.0"` フィールド必須
3. **Polling**: 毎 tick で `thread/list` (limit=50, sortKey=updated_at) を呼び出し
   - Thread status 変化 (idle → active, active → idle 等) を検出
   - `cwd` フィルタで特定 pane の thread のみ取得可能 (T-119)
4. **Notification 処理**: `thread/list` response 前に到着する notification を drain
   - `turn/started` → `CodexRawEvent { event_type: "turn.started" }`
   - `turn/completed` → `CodexRawEvent { event_type: "turn.{status}" }` (completed/interrupted/failed)
   - `thread/status/changed` → `CodexRawEvent { event_type: "thread.{type}" }` (idle/active/systemError)
5. **Graceful degradation**: spawn 失敗 or handshake 失敗 → `None`, fallback path へ

##### Fallback path: Capture-based NDJSON extraction

App Server が利用不可の場合のみ使用 (`codex` 未インストール、認証失敗等):
1. Poller が Codex と判定した pane の tmux capture lines をスキャン
2. `{"type": "..."}` 形式の JSON lines を parse し `CodexRawEvent` に変換
3. `CodexCaptureTracker` で content-based fingerprint dedup (cross-tick)

##### poll_tick Step 6a

```
if app_server.is_alive():
    events = app_server.poll_threads()
    → codex_source.ingest(events)
    [app_server alive → capture skip]
else:
    app_server = None  // clear dead client
    → capture fallback for Codex-detected panes
```

##### pane_id correlation (T-119 → T-120)

App Server の thread は `cwd` を持つが `pane_id` を持たない。tmux pane も `current_path` (cwd) を持つ。
この対応を取ることで、Codex thread の event に `pane_id` を付与し、pane-level deterministic 検出を実現する。

```
thread/list response:
  [{ id: "thr_abc", status: { type: "active" }, cwd: "/Users/me/project" }]

tmux list-panes:
  [{ pane_id: "%5", current_path: "/Users/me/project" }]

→ cwd match → CodexRawEvent.pane_id = "%5"
→ translate() → SourceEventV2.pane_id = Some("%5")
→ daemon project_pane() が実行される
→ list_panes で signature_class: Deterministic, evidence_mode: Deterministic
```

マッチング戦略:
- `thread/list` の `cwd` と `PaneSnapshot.current_path` を正規化して比較
- 同一 cwd に複数 pane がある場合: Codex process hint がある pane を優先
- 同一 cwd に複数 thread がある場合: status=active な thread を優先、同率なら updatedAt 最新
- マッチしない thread: `pane_id = None` のまま (session-level のみ投影)

#### Detection Accuracy Hardening (MVP)

##### 問題
Poller のヒューリスティック検出は `pane_title` / `current_cmd` / `process_hint` の3シグナルのみで判定するため:
1. **偽陽性**: tmux の `pane_title` は agent 終了後も残存 → stale title が検出を誤発火
2. **偽陰性**: Claude Code / Codex は `current_cmd = "node"` で動作し、`pane_title` もエージェント名を含まない場合がある

##### Capture-based detection (第4シグナル)
- `PaneMeta` に `capture_lines: Vec<String>` を追加し、`detect()` で capture 内容を provider-specific tokens でスキャン
- Weight: `WEIGHT_POLLER_MATCH` (0.78) — title_match (0.66) より高く、cmd_match (0.86) より低い
- MVP capture tokens（レビュー採択: 偽陽性低減のため十分に specific なトークンを使用）:
  - Claude: `["claude code", "╭ Claude Code"]`（bare `╭` は lazygit/btop 等の TUI と衝突するため不採用）
  - Codex: `["codex>"]`（bare `codex` は git log 等で偽陽性のため不採用）
- `ProviderDetectDef` に `capture_tokens: &'static [&'static str]` を追加
- `DetectResult` に `capture_match: bool` を追加し、event payload に `"capture_match"` として伝搬
- `extract_signature_inputs()` で payload の `capture_match` を読み取り `poller_match` に OR 合成

##### Stale title 抑制
- title_match のみ（process_hint/cmd_match/capture_match いずれも false）かつ `current_cmd` が shell の場合、検出結果を `None` に抑制
- Shell list: `zsh`/`bash`/`fish`/`sh`/`dash`/`nu`/`pwsh`/`tcsh`/`csh`/`ksh`/`ash`（レビュー採択: nushell/PowerShell Core/tcsh/ksh/ash 追加）
- 比較は case-insensitive + basename 抽出 + login-shell prefix 除去（`/usr/local/bin/fish` → `fish`, `-zsh` → `zsh`）
- Rationale: 実際にエージェントが動いている場合は capture content にエージェント固有パターンが出るため、title_match + shell cmd のみでは stale title の可能性が高い

##### Per-pane activity_state + provider
- `PaneRuntimeState` に `activity_state: ActivityState` と `provider: Option<Provider>` を追加（レビュー採択: Option で unmanaged pane の "未検出" を表現）
- `project_pane()` で `event.event_type` → `parse_activity_state()` で投影
- `list_panes` API 応答に `activity_state` と `provider` フィールドを追加
- 既存の session-level activity_state に加え、pane-level でも Running/Idle/WaitingApproval 等が参照可能に

## Appendix (Post-MVP Hardening; non-blocking for Phase 1-2)

### A1) Cursor two-watermark + ack delivery
- `fetched_cursor` / `committed_cursor` 二水位
- `delivery_token` + `gateway.ack_delivery` idempotency
- ack timeout/retry/redelivery (`ack_timeout=2s`, `max_attempts=5`)

### A2) Invalid cursor recovery numeric contract
- checkpoint: `30s` or `500 events`
- safe rewind: `min(10m, 10,000 events)`
- streak-based full resync (`>=3 in 60s`)
- dedup retention: `rewind_window + 120s`

### A3) UDS trust boundary + source registry lifecycle
- peer credential check (`SO_PEERCRED` / `getpeereid`)
- runtime nonce / protocol version check
- registry states: `pending/active/stale/revoked`
- socket rotation policy (re-register)

### A4) Supervisor runtime contract (strict)
- startup readiness gate (dependency-aware)
- exponential backoff + jitter
- failure budget (`5 failures / 10m`)
- hold-down (`5m`) + escalate

### A5) Binding concurrency hardening
- single-writer projection
- `state_version` CAS update
- CAS conflict retry
- ordering key `(event_time, ingest_seq)`

### A6) Ops guardrail manager and alerts
- alert levels: `warn/degraded/escalate`
- rolling window evaluation
- `list_alerts` API + `alert_ledger_v1`
- auto-resolve / operator-ack resolve policy

### A7) Snapshot / restore contract
- periodic snapshot (`15m`) + shutdown snapshot
- restore dry-run before canary
- snapshot metadata persistence

### A8) Extended error taxonomy / tests
- post-MVP error codes:
  - `invalid_cursor`, `ack_timeout`, `ack_retry_exhausted`, `unknown_delivery_token`
  - `peer_uid_mismatch`, `source_registry_miss`, `runtime_nonce_mismatch`
  - `source_not_active`, `source_revoked`
  - `binding_cas_conflict`, `rewind_window_exceeded`, `slo_breach`
- post-MVP test additions:
  - ack state machine
  - source registry lifecycle
  - supervisor hold-down
  - snapshot restore dry-run
