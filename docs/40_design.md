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
  - Codex: `appserver > poller`
  - Claude: `hooks > poller`
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
