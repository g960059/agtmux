# State Engine Specification

状態推定のコアロジック。特定の terminal backend や IO に依存しない pure な設計。
Go POC のアルゴリズム構造 (evidence scoring + precedence + TTL) は参考にするが、
weight/confidence 数値はテスト駆動で独自に決定する。

## Core Principle: Engine は Evidence のみ受け取る

```
Orchestrator: PaneMeta → detectors.detect() → builders.build_evidence()
Engine:       &[Evidence] → resolve() → ResolvedActivity
```

Engine を pure scorer にすることで、任意の evidence を渡してテスト可能。
PaneMeta の解釈は adapt layer (ProviderDetector + EvidenceBuilder) が担当する。

## Key Types

```rust
// types/activity.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ActivityState {
    Running, WaitingInput, WaitingApproval, Idle, Error, Unknown,
}

// types/provider.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Provider {
    Claude, Codex, Gemini, Copilot,
}

// types/source.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SourceType { Hook, Api, File, Poller }

// types/evidence.rs
pub struct Evidence {
    pub provider: Provider,
    pub kind: EvidenceKind,
    pub signal: ActivityState,
    pub weight: f64,
    pub confidence: f64,
    pub timestamp: DateTime<Utc>,
    pub ttl: Duration,
    pub source: SourceType,
    pub reason_code: String,
}

pub enum EvidenceKind {
    HookEvent(String),      // hook event name
    ApiNotification(String), // API notification type
    FileChange(String),      // file path
    PollerMatch(String),     // matched pattern
}
```

## Engine

```rust
// engine/mod.rs
pub struct Engine { config: EngineConfig }

impl Engine {
    pub fn resolve(&self, evidence: &[Evidence], now: DateTime<Utc>) -> ResolvedActivity;
}

pub struct ResolvedActivity {
    pub state: ActivityState,
    pub confidence: f64,
    pub source: SourceType,
    pub reason_code: String,
}
```

### EngineConfig

```rust
pub struct EngineConfig {
    pub min_score: f64,                // 0.35
    pub running_enter_score: f64,      // 0.62
    pub min_stable_duration: Duration, // 1500ms
    pub default_evidence_ttl: Duration, // 90s
    pub high_confidence_ttl: Duration,  // 180s
    pub low_confidence_ttl: Duration,   // 30s
    pub strong_source_bonus: f64,      // 0.15
    pub weak_source_multiplier: f64,   // 0.75
}
```

### Resolve Flow

```
Engine.resolve(evidence, now)
├── 1. TTL 切れの evidence を除外
├── 2. 各 ActivityState ごとに weighted score を集計
│      score += evidence.weight * evidence.confidence
│      source bonus: Hook/Api → +strong_source_bonus
│      source penalty: Poller → ×weak_source_multiplier
├── 3. precedence と score でソート
│      Error > WaitingApproval > WaitingInput > Running > Idle > Unknown
├── 4. threshold check
│      min_score, running_enter_score (epsilon 1e-9 で比較)
└── Output: ResolvedActivity
```

## Multi-Layer Source Architecture

状態検知は4層の source から evidence を収集する。

```
StateSource (trait)
├── HookSource    — agent の公式 hook/notify (event-driven, push)
├── ApiSource     — agent の JSON-RPC API (event-driven, push)
├── FileSource    — session ファイル監視 (kqueue/inotify, near-realtime)
└── PollerSource  — terminal output pattern matching (500ms poll)
```

### Provider 別 Source 利用可能性

| Source | Claude | Codex | Gemini | Copilot |
|--------|--------|-------|--------|---------|
| **Hook** | 17 events (公式 hooks) | 1 event (notify: turn-complete のみ) | - | - |
| **API** | - | app-server WebSocket (全 lifecycle) | - | - |
| **File** | `~/.claude/projects/*/sessions-index.json` | `~/.codex/sessions/**/*.jsonl` | - | - |
| **Poller** | capture-pane + pattern | capture-pane + pattern | capture-pane + pattern | capture-pane + pattern |

### Graceful Degradation

daemon は起動時に各 provider の利用可能な source を probe し、最上位の source から順に使用する。
hook/API が未設定でも poller だけで動作する（精度は低下する）。

```
Claude pane 検出
├── hooks 設定済み? → HookSource (SourceType::Hook, weight=1.0)
├── ~/.claude/ 読み取り可? → FileSource (SourceType::File, weight=0.70)
└── 常に有効 → PollerSource (SourceType::Poller, weight=0.55)

Codex pane 検出
├── app-server 起動中? → ApiSource (SourceType::Api, weight=0.98)
├── notify 設定済み? → HookSource (SourceType::Hook, weight=0.95)
├── ~/.codex/ 読み取り可? → FileSource (SourceType::File, weight=0.70)
└── 常に有効 → PollerSource (SourceType::Poller, weight=0.55)
```

## Provider Adapter Traits (Composition)

provider の capabilities を細粒度 trait で表現する。Gemini のように poller しかない provider は Detector + EvidenceBuilder だけ実装すればよい。

```rust
// adapt/mod.rs
pub trait ProviderDetector: Send + Sync {
    fn id(&self) -> Provider;
    fn detect(&self, meta: &PaneMeta) -> Option<f64>;
}

pub trait EvidenceBuilder: Send + Sync {
    fn provider(&self) -> Provider;
    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence>;
}

pub trait EventNormalizer: Send + Sync {
    fn provider(&self) -> Provider;
    fn normalize(&self, signal: &RawSignal) -> Option<NormalizedState>;
}
```

### Provider 別 trait 実装マトリクス

| Trait | Claude | Codex | Gemini | Copilot |
|-------|--------|-------|--------|---------|
| **ProviderDetector** | yes | yes | yes | yes |
| **EvidenceBuilder** | yes | yes | yes | yes |
| **EventNormalizer** | yes (hooks) | yes (API/notify) | no | no |

新 provider を追加するとき、実装が必要な trait だけ impl する。
内蔵 provider (Claude, Codex, Gemini, Copilot) は TOML + adapter struct で提供。

## 宣言的 Provider 定義 (TOML)

signal→evidence mapping を TOML で宣言的に定義する。Phase 0 から導入。

```toml
# providers/claude.toml
[detection]
agent_type = "claude"
cmd_tokens = ["claude"]

[[signals]]
pattern = ["approval", "waiting_approval", "needs_approval", "permission"]
activity = "WaitingApproval"
weight = 0.95
confidence = 0.92

[[signals]]
pattern = ["waiting_input", "input_required", "await_user", "prompt"]
activity = "WaitingInput"
weight = 0.90
confidence = 0.88
```

`include_str!` で compile-time 埋め込み。`--config` で runtime 上書き可能。

### Provider Detection

```rust
fn detect(meta: &PaneMeta, provider_def: &ProviderDef) -> Option<f64> {
    if meta.agent_type matches provider_def.agent_type  => Some(1.0)
    if meta.current_cmd contains provider_def.cmd_tokens => Some(0.86)
    if meta.pane_title contains provider_def.agent_type  => Some(0.66)
    else                                                  => None
}
```

## Claude Hook Events → Evidence Mapping

| Hook Event | → Activity | SourceType |
|------------|-----------|------------|
| `PreToolUse` | Running | Hook |
| `PostToolUse` | Running | Hook |
| `Notification(permission_prompt)` | WaitingApproval | Hook |
| `Notification(idle_prompt)` | WaitingInput | Hook |
| `Stop` | Idle | Hook |
| `SessionStart` | Idle | Hook |
| `SessionEnd` | Idle | Hook |
| `SubagentStart` | Running | Hook |

## Codex API Events → Evidence Mapping

| API Notification | → Activity | SourceType |
|-----------------|-----------|------------|
| `turn/started` | Running | Api |
| `turn/completed` | Idle | Api |
| `item/commandExecution/requestApproval` | WaitingApproval | Api |
| `item/fileChange/requestApproval` | WaitingApproval | Api |
| `turn/failed` | Error | Api |

## SourceEvent (daemon 側で使う型)

```rust
// source.rs
pub trait StateSource: Send + 'static {
    fn source_type(&self) -> SourceType;
}

pub enum SourceEvent {
    RawSignal {
        pane_id: String,
        event_type: String,
        source: SourceType,
        payload: String,
        timestamp: DateTime<Utc>,
    },
    Evidence {
        pane_id: String,
        evidence: Vec<Evidence>,
    },
    TopologyChange(TopologyEvent),
}
```

## Activity State Precedence

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActivityState {
    Unknown          = 0,  // 最低
    Idle             = 1,
    Running          = 2,
    WaitingInput     = 3,
    WaitingApproval  = 4,
    Error            = 5,  // 最高優先
}
```

## Attention Derivation

```rust
fn derive_attention_state(
    activity: ActivityState,
    reason_code: &str,
    last_event_type: &str,
    last_event_at: Option<DateTime>,
    last_interaction_at: Option<DateTime>,
    updated_at: DateTime,
) -> AttentionResult {
    match activity {
        WaitingInput    => ActionRequiredInput,
        WaitingApproval => ActionRequiredApproval,
        Error           => ActionRequiredError,
        _ if is_completion_signal(reason_code, last_event_type)
          && !contains("input", "approval")
                        => InformationalCompleted,
        _               => None,
    }
}
```

Administrative events (attention を発火しない):
- `wrapper-start`, `wrapper-exit`
- `action.*` (except `action.view-output`)

## Async Pipeline (daemon 内)

`tokio::select!` + mpsc/broadcast channels:

```
Sources (async tasks) → mpsc<SourceEvent> → Orchestrator → broadcast<StateNotification> → Clients
```

Orchestrator の main loop:
1. `mpsc` から `SourceEvent` を受信
2. `RawSignal` → `EventNormalizer` → `Evidence`
3. pane ごとに evidence window を蓄積
4. `Engine::resolve(&evidence_window)` で状態を決定
5. `derive_attention_state()` で attention を導出
6. `broadcast` で全 client に通知
