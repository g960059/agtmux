# Architecture (mutable)

## System Context
- Actors:
  - User (CLI/TUI/GUI)
  - Runtime operator (開発者/保守者)
- External systems:
  - tmux server
  - Claude hooks stream
  - Codex app-server stream
  - provider local files（将来/任意）
  - local SQLite

## Components
- C-001: `agtmux-runtime-supervisor`
  - runtime の起動順制御・ヘルスチェック・再起動担当
- C-002: `agtmux-daemon-v5`
  - resolver + read model + client API
- C-003: `agtmux-gateway`
  - 複数 source server から pull 集約し cursor 管理
- C-004: `agtmux-source-codex-appserver`（Deterministic）
  - Codex lifecycle event を正規化
- C-005: `agtmux-source-claude-hooks`（Deterministic）
  - Claude hook event を正規化
- C-006: `agtmux-source-poller`（Heuristic fallback）
  - v4 poller pattern 判定を再利用
- C-007: `agtmux-source-file`（Optional/後段）
  - ファイル観測系 source を分離追加
- C-008: `agtmux-core-v5`
  - 型・tier resolver・attention導出（IOなし）
- C-009: `capability registry`（logical component）
  - provider/source capability を宣言管理し、将来 source を追加しても daemon の責務を増やさない
- C-010: `agtmux-title-resolver`（reusable component）
  - pane/session handshake を使って canonical session title を解決し、session tile 表示に供給

## Data Flow (key flows)
- Flow-001: 新規 agent session 起動（managed + deterministic）
  - UI launch -> source deterministic event -> gateway -> daemon -> `presence=managed`, `evidence_mode=deterministic` -> client push
- Flow-002: 既存 pane 検出（presence判定）
  - daemon refresh(tmux capture) -> agent session signature 判定
  - signatureあり: `managed` + initial `heuristic` mode（後続 handshake で `deterministic` へ昇格）
  - signatureなし: `unmanaged` + `none`（non-agent）
- Flow-003: deterministic outage -> fallback -> re-promotion
  - source health stale/down -> poller accepted -> deterministic fresh復帰 -> winner即再昇格
- Flow-004: source health 更新
  - supervisor/gateway probe -> health table更新 -> daemon admissibility 判定へ反映
- Flow-005: title handshake and session tile rendering
  - deterministic event で pane_id <-> session_key を確立 -> title resolver が canonical session name を解決 -> UI は handshake 完了paneで canonical title を優先表示

### Topology
```
[TUI/GUI]
   |
[agtmux-runtime-supervisor]
   |- [agtmux-daemon-v5]
   |- [agtmux-gateway]
   |- [agtmux-source-codex-appserver]
   |- [agtmux-source-claude-hooks]
   '- [agtmux-source-poller]

source-* --pull--> gateway --pull--> daemon --push--> clients
```

## Storage / State
- Persisted data:
  - `events_raw_v2`（append-only）
  - `session_state_v2`（session winner 最新）
  - `pane_state_v2`（pane winner 最新）
  - `source_health_v2`（source health 最新）
- Runtime state:
  - provider/source ごとの watermark
  - dedup cache（event_id/hash）
  - provider arbitration watermark
  - pane presence (`managed`/`unmanaged`)
  - pane evidence mode (`deterministic`/`heuristic`/`none`)

## Security Model (high-level)
- AuthN/AuthZ assumptions:
  - ローカル単一ユーザー実行を前提（UDS permissions）
- Secret handling:
  - provider credential は source server 側でのみ扱い、daemon へ渡さない
- Input validation:
  - `pane_id` 一貫性、payload schema、source/provider 整合を検証

## Observability
- Logs:
  - ingest reject reason（`source_inadmissible` など）
  - source suppression reason（higher-priority source）
  - fallback/re-promotion 発火
- Metrics:
  - source health state counts
  - winner tier/source distribution
  - state latency p95
  - attention precision/recall（labeling時）
- Tracing:
  - event_id 単位で `source -> gateway -> daemon -> client` を追跡

## Risks & Tradeoffs
- R-001: 外部プロセス化で運用面の複雑度が上がる
  - Tradeoff: その代わりテスト境界と障害分離が明確になる
- R-002: source protocol 変更による ingest break
  - Tradeoff: source server 単位で影響を閉じ込められる
- R-003: poller fallback 品質の過信
  - Tradeoff: deterministic unavailable 時のみ利用することで誤判定の影響を限定
