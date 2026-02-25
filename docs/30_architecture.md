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

## Scope tags
- `[MVP]`: Phase 1-2 の実装ブロッカー
- `[Post-MVP]`: 設計保持（Phase 1-2 非ブロッカー）

## Components
- C-001 `[MVP]`: `agtmux-runtime-supervisor`
  - runtime の起動順制御・ヘルスチェック・再起動担当（strict readiness/backoff は Post-MVP）
- C-002 `[MVP]`: `agtmux-daemon-v5`
  - resolver + read model + client API
- C-003 `[MVP]`: `agtmux-gateway`
  - 複数 source server から pull 集約し cursor 管理（MVPは committed cursor のみ）
- C-004 `[MVP]`: `agtmux-source-codex-appserver`（Deterministic）
  - Codex lifecycle event を正規化
- C-005 `[MVP]`: `agtmux-source-claude-hooks`（Deterministic）
  - Claude hook event を正規化
- C-006 `[MVP]`: `agtmux-source-poller`（Heuristic fallback）
  - v4 poller pattern 判定を再利用
- C-007 `[Post-MVP]`: `agtmux-source-file`（Optional/後段）
  - ファイル観測系 source を分離追加
- C-008 `[MVP]`: `agtmux-core-v5`
  - 型・tier resolver・attention導出（IOなし）
- C-009 `[MVP]`: `capability registry`（logical component）
  - provider/source capability を宣言管理し、将来 source を追加しても daemon の責務を増やさない
- C-010 `[MVP]`: `agtmux-title-resolver`（reusable component）
  - pane/session handshake を使って canonical session title を解決し、session tile 表示に供給
- C-011 `[MVP]`: `binding registry`（logical component）
  - pane-first の binding state machine を保持（`pane_instance` と `session_key` の時系列 link）
- C-012 `[MVP]`: `pane signature classifier`（logical component）
  - `signature_class/reason/confidence` を算出し、presence 判定の入力を提供
- C-013 `[Post-MVP]`: `ops guardrail manager`（logical component）
  - SLO window判定、alert発火、snapshot/restore 実行状態を管理
- C-014 `[Post-MVP]`: `source registry manager`（logical component）
  - source endpoint の登録/失効/復帰（`pending/active/stale/revoked`）を管理

## Data Flow (key flows)
- Flow-001 `[MVP]`: 新規 agent session 起動（managed + deterministic）
  - UI launch -> source deterministic event -> gateway -> daemon -> `presence=managed`, `evidence_mode=deterministic` -> client push
- Flow-002 `[MVP]`: 既存 pane 検出（presence判定）
  - daemon refresh(tmux capture + ps hints) -> pane signature classifier 判定
  - deterministic signatureあり: `managed` + `deterministic`
  - heuristic signatureあり: `managed` + `heuristic`
  - signatureなし / no-agent連続観測: `unmanaged` + `none`
- Flow-003 `[MVP]`: deterministic outage -> fallback -> re-promotion
  - source health stale/down -> poller accepted -> deterministic fresh復帰 -> winner即再昇格
- Flow-004 `[MVP]`: source health 更新
  - supervisor/gateway probe -> health table更新 -> daemon admissibility 判定へ反映
- Flow-005 `[MVP]`: title handshake and session tile rendering
  - deterministic event で pane_instance <-> session_key を確立 -> title resolver が canonical session name を解決 -> UI は handshake 完了paneで canonical title を優先表示
- Flow-006 `[Post-MVP]`: cursor commit and replay safety
  - source pull -> gateway に `fetched_cursor` を記録 -> daemon projection 完了後 ack -> `committed_cursor` を前進
  - daemon crash/restart 時は `committed_cursor` から再pull（at-least-once + dedup）
- Flow-007 `[MVP]`: pane reuse / rebinding
  - 同一 `pane_id` が再利用されたら `generation` を更新して新 `pane_instance` として扱う
  - 旧 `pane_instance` は grace 期間 `tombstone` 化して誤結合を防止
- Flow-008 `[MVP]`: heuristic hysteresis guard
  - poller idle は安定窓通過後に確定、running は interaction窓で昇格、hint消失で時間降格
  - title-only / wrapper 誤判定は classifier guard で遮断
- Flow-009 `[Post-MVP]`: UDS trust admission
  - source 接続受理時に peer credential を検証（same UID）し、source registry（source_kind/socket_path/owner_uid）一致時のみ accept
  - source.hello の runtime nonce 不一致は即 reject（fail-closed）
- Flow-010 `[Post-MVP]`: snapshot / restore readiness
  - supervisor が `15m` ごとに state snapshot を作成し、shutdown 前にも最終 snapshot を取得
  - canary 前に restore dry-run を実行し、cursor/binding/state の整合を検証
- Flow-011 `[Post-MVP]`: supervisor start/restart contract
  - 起動順序は source -> gateway -> daemon -> UI
  - readiness 未達は次段起動を停止（fail-closed）し、指数バックオフ（1s->30s, jitter 20%）で再試行
  - `10m` 内 `5` 回失敗で hold-down `5m` に入り `escalate` を発火
- Flow-012 `[Post-MVP]`: ack timeout and redelivery
  - gateway は batch ごとに `delivery_token` を採番し inflight を保持
  - `ack_timeout=2s` 超過時は同一 token を再配送（idempotent）
  - `max_attempts=5` 超過時は source を `degraded` へ遷移し `ack_retry_exhausted` を記録
- Flow-013 `[Post-MVP]`: source registry lifecycle
  - `source.hello` 成功で `pending -> active`
  - heartbeat timeout（30s）または socket 不達で `active -> stale`
  - operator revoke で `* -> revoked`
  - socket rotation は同一 `source_kind + owner_uid` の再登録で `stale -> active`
- Flow-014 `[Post-MVP]`: binding serial projection
  - daemon projection loop を single-writer とし、`state_version` CAS で pane/session state を更新
  - 競合時は最新 state を再読込して再評価し、古い event での巻き戻りを防止

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
- Section gate:
  - `[MVP]` のみ Phase 1-2 実装対象
  - `[Post-MVP]` は設計保持（Phase 1-2 非ブロッカー）
- Persisted data:
  - `[MVP]` `events_raw_v2`（append-only）
  - `[MVP]` `session_state_v2`（session winner 最新）
  - `[MVP]` `pane_state_v2`（pane winner 最新）
  - `[MVP]` `binding_link_v2`（pane_instance <-> session_key の時系列 link）
  - `[MVP]` `cursor_state_v2`（MVPは committed cursor 中心）
  - `[MVP]` `source_health_v2`（source health 最新）
  - `[Post-MVP]` `state_snapshot_v2`（periodic + shutdown snapshot metadata）
  - `[Post-MVP]` `source_registry_v1`（source lifecycle）
  - `[Post-MVP]` `alert_ledger_v1`（warn/degraded/escalate events）
- Runtime state:
  - `[MVP]` provider/source ごとの watermark
  - `[MVP]` dedup cache（event_id/hash）
  - `[MVP]` provider arbitration watermark
  - `[MVP]` `pane_instance` index（`pane_id,generation,birth_ts`）
  - `[MVP]` pane signature cache（`signature_class`, `signature_reason`, `signature_confidence`）
  - `[MVP]` pane presence (`managed`/`unmanaged`)
  - `[MVP]` pane evidence mode (`deterministic`/`heuristic`/`none`)
  - `[Post-MVP]` SLO evaluator windows（rolling 10m, min samples 200）
  - `[Post-MVP]` delivery inflight table（`delivery_token`, attempts, lease_deadline）

## Security Model (high-level)
- Section gate:
  - `[MVP]`: local single-user + schema validation を基本とする
  - `[Post-MVP]`: peer credential / source registry / nonce を追加する
- AuthN/AuthZ assumptions:
  - `[MVP]` ローカル単一ユーザー実行を前提にする
  - `[Post-MVP]` same-UID 偽装注入対策として UDS peer credential 検証を追加する
  - `[Post-MVP]` 接続受理条件は `peer_uid == runtime_uid` かつ source registry 一致（`source_kind + socket_path + owner_uid`）
- Registry authority:
  - `[Post-MVP]` source registry 更新主体は runtime supervisor（自動遷移）+ operator command（manual revoke/register）に限定する
- Secret handling:
  - `[MVP]` provider credential は source server 側でのみ扱い、daemon へ渡さない
- Input validation:
  - `[MVP]` `pane_id` 一貫性、payload schema、source/provider 整合を検証
  - `[Post-MVP]` source.hello の runtime nonce / protocol version を検証し、不一致は fail-closed

## Observability
- Section gate:
  - `[MVP]` は local debug に必要な最小可観測性
  - `[Post-MVP]` は運用 hardening 用
- Logs:
  - `[MVP]` ingest reject reason（`source_inadmissible` など）
  - `[MVP]` source suppression reason（higher-priority source）
  - `[MVP]` fallback/re-promotion 発火
  - `[MVP]` pane signature transition（`none -> heuristic -> deterministic` など）
  - `[Post-MVP]` UDS admission reject reason（`peer_uid_mismatch`, `source_registry_miss`, `runtime_nonce_mismatch`）
  - `[Post-MVP]` snapshot create/restore dry-run result
  - `[Post-MVP]` supervisor restart / hold-down transition
  - `[Post-MVP]` source registry lifecycle transition
  - `[Post-MVP]` ack timeout / redelivery / retry exhausted
- Metrics:
  - `[MVP]` source health state counts
  - `[MVP]` winner tier/source distribution
  - `[MVP]` state latency p95（deterministic <= 2.0s / fallback <= 5.0s）
  - `[MVP]` hop latency p95（source/gateway/daemon/client）
  - `[MVP]` binding churn（rebind/tombstone）
  - `[MVP]` signature class distribution / reason top-k / presence flap rate
  - `[MVP]` attention precision/recall（labeling時）
  - `[Post-MVP]` cursor rewind / duplicate / drop
  - `[Post-MVP]` SLO breach windows（warn/degraded/escalate）
  - `[Post-MVP]` snapshot age / restore success rate
  - `[Post-MVP]` supervisor restart count / hold-down count
  - `[Post-MVP]` ack timeout rate / retry attempts / retry exhausted count
  - `[Post-MVP]` source registry state counts（pending/active/stale/revoked）
- Tracing:
  - `[MVP]` event_id 単位で `source -> gateway -> daemon -> client` を追跡
  - `[Post-MVP]` source connection 単位で `admission -> hello -> pull_events` を追跡
  - `[Post-MVP]` delivery_token 単位で `pull_events -> delivery -> ack/redelivery` を追跡

## Risks & Tradeoffs
- R-001: 外部プロセス化で運用面の複雑度が上がる
  - Tradeoff: その代わりテスト境界と障害分離が明確になる
- R-002: source protocol 変更による ingest break
  - Tradeoff: source server 単位で影響を閉じ込められる
- R-003: poller fallback 品質の過信
  - Tradeoff: deterministic unavailable 時のみ利用することで誤判定の影響を限定
- R-004: pane-first + generation 導入で状態遷移が複雑化
  - Tradeoff: pane再利用/移動/多重紐付けの誤結合リスクを下げ、観測再現性を上げられる
- R-005: heuristic signature 閾値設定が過敏だと managed/unmanaged がフラップする
  - Tradeoff: v4/POC由来の閾値固定（8s/45s/idle安定窓）で初期挙動を安定化できる
- R-006: supervisor/registry/ack 再送ポリシー追加で運用ロジックが増える
  - Tradeoff: 障害時の挙動が deterministic になり、復旧判断を自動化できる
