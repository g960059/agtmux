# Design (mutable; changes common)

## Interfaces / APIs

### 1) Source Server -> Gateway
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
    "source_health":{"status":"healthy|degraded|down","checked_at":"2026-..."}
  }
}
```

### 2) Gateway -> Daemon
- Transport: UDS JSON-RPC
- Method: `gateway.pull_events`
- Poll interval (default): daemon 250ms / gateway 200ms

### 3) Daemon -> Clients
- Pull:
  - `list_panes`
  - `list_sessions`
  - `list_source_health`
- Push:
  - `state_changed`
  - `summary_changed`

### 4) Compatibility hook (transition aid)
- Method: `ingest_source_event`
- Purpose: bridge/runtime wiring の段階移行時のみ使用（最終的には gateway pull へ統合）

### 5) MVP deterministic sources (fixed)
- Codex: `agtmux-source-codex-appserver`（appserver events）
- Claude: `agtmux-source-claude-hooks`（hooks events）
- Note: Codex hooks など将来 capability は source server を増やして統合する。

### 6) Reuse strategy (from v4)
- Reuse as crate/module:
  - poller pattern matching core
  - source health state transition logic
  - title resolution logic (canonical session index + binding history)
- Do not reuse as-is:
  - v4 orchestrator monolith (責務分離の観点で再構成する)
  - v4 store schema (v5 `*_v2` に合わせて再設計する)

## Data Model

### Canonical entities
```rust
pub enum EvidenceTier { Deterministic, Heuristic }
pub enum PanePresence { Managed, Unmanaged } // agent session 有無
pub enum EvidenceMode { Deterministic, Heuristic, None } // 判定経路

pub struct SourceEventV2 {
    pub event_id: String,
    pub provider: Provider,
    pub source_kind: SourceKind,
    pub tier: EvidenceTier,
    pub observed_at: DateTime<Utc>,
    pub session_key: String,
    pub pane_id: Option<String>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub confidence: f64,
}

pub struct SessionRuntimeState {
    pub session_key: String,
    pub presence: PanePresence,
    pub evidence_mode: EvidenceMode,
    pub deterministic_last_seen: Option<DateTime<Utc>>,
    pub winner_tier: EvidenceTier,
    pub activity_state: ActivityState,
    pub activity_source: SourceKind,
    pub updated_at: DateTime<Utc>,
}
```

### Source health
```rust
pub enum SourceHealthState { Healthy, Degraded, Down }
pub struct SourceHealthRecord {
    pub source_kind: SourceKind,
    pub state: SourceHealthState,
    pub checked_at: DateTime<Utc>,
    pub reason: String,
}
```

### Persistence schema (v2)
- `events_raw_v2`:
  - key: `(provider, source_kind, event_id)`
  - fields: payload, observed_at, tier
- `session_state_v2`:
  - key: `session_key`
  - fields: presence, evidence_mode, winner_tier, activity_state, updated_at
- `pane_state_v2`:
  - key: `pane_id`
  - fields: presence, evidence_mode, activity_source, attention_state, attention_reason, updated_at
- `source_health_v2`:
  - key: `source_kind`
  - fields: state, checked_at, reason

## Resolver and Arbitration
- Deterministic freshness:
  - fresh: `now - deterministic_last_seen <= 3s`
  - stale: `> 3s`
  - down: `> 15s` or health down
- Winner selection:
  1. dedup (`provider + session_key + event_id`)
  2. split deterministic/heuristic
  3. deterministic fresh があれば deterministic tier 内で解決
  4. それ以外は heuristic tier 内で解決
  5. fresh deterministic 再到達で即時 re-promotion
- Presence rule:
  - deterministic/heuristic の切り替えは `presence` を変更しない。
  - `presence=managed` は agent session がある限り維持する。
- Intra-tier tie break:
  - score（weight * confidence + source bonus/penalty）
  - 同点は新しい `observed_at`、次に source rank

## Pane/Session Title Resolution
- Handshake completion:
  - deterministic source event で `session_key` と `pane_id` の関連が確立し、最新関連が有効であること
- Title priority (highest first):
  1. canonical agent session name（handshake 完了時）
  2. bound title history（同一 pane binding 継続時）
  3. live pane title（generic/placeholder は除外）
  4. canonical path-based title
  5. deterministic fallback title
- UI rule:
  - handshake 完了 pane は session tile で canonical agent session name を優先表示する

## Error Handling
- Error taxonomy:
  - `invalid_source_event`
  - `missing_event_time`
  - `source_unsupported_for_provider`
  - `source_inadmissible`
  - `source_rank_suppressed`
  - `normalizer_recoverable_error`
  - `late_event`
- User-visible messages:
  - API は JSON-RPC error code + reason string を返す
  - UI は「監視継続可否」に絞って表示（詳細は logs/diagnostics）

## Compatibility & Fallbacks
- Default: breaking change を許容（v5はgreenfield）
- Exceptions:
  - v4 poller-only equivalent の動作は回帰禁止
  - `status/tui/tmux-status` の主要表示意味は維持

## Test Strategy (design-level)
- Unit:
  - tier resolver（fresh/stale/down境界、re-promotion）
  - source admissibility（priority + health）
  - dedup/watermark/late-event
- Integration:
  - existing managed(heuristic) pane + deterministic handshakeでmode昇格
  - existing unmanaged(non-agent) pane の非汚染確認
  - new managed pane detection via signature（deterministic未接続）
  - managed pane の deterministic handshake 到達で mode 昇格
  - source outage/recovery and suppress behavior
- E2E/Replay:
  - `hook_to_poller_fallback`
  - deterministic drop -> fallback -> re-promotion
  - mixed provider with independent health
- Accuracy gate:
  - v4下限を継承（Dev: weighted F1 >= 0.88）
  - fallback専用の品質指標（約85%前提）を別途固定
