# Spec (mutable)

## Scope
- In:
  - 2層 winner モデル（Deterministic/Heuristic）の仕様固定
  - source server + gateway + daemon + UI supervisor の責務分離
  - v4 poller を fallback server として再利用する方針
  - `managed/unmanaged` を agent session 有無の軸として固定
  - managed 表記を `agents` に統一し、badge は unmanaged のみ表示
  - docs-first 実行管理（tasks/progress/ADR/review pack）
- Out:
  - v5 実装コードそのもの（本ターンでは docs 完成まで）
  - provider別UI最適化
  - 分散配置・リモートクラスタ運用

## Terminology (naming)
- Axis A: Agent session presence
  - `managed` = agent session がある
  - `unmanaged` = agent session がない（zsh 等）
- Axis B: Evidence mode
  - `deterministic` = hooks/appserver handshake が有効
  - `heuristic` = poller中心で推定（deterministic未接続/不在）
- Recommended names (session/pane):
  - `agent_session_deterministic`（hooks/appserverありのagent session）
  - `agent_session_heuristic`（hooks/appserverなしのagent session）
  - `pane_managed_deterministic`（deterministic agent session と紐づいた pane）
  - `pane_managed_heuristic`（poller中心の managed pane）
  - `pane_unmanaged_non_agent`（agent sessionがない pane）
- Suggested display labels (human-readable):
  - `Agent session (deterministic)`
  - `Agent session (heuristic)`
  - `Managed pane (deterministic)`
  - `Managed pane (heuristic fallback)`
  - `Unmanaged pane (non-agent)`

## Functional Requirements
- FR-001: resolver は tier winner を採用し、fresh deterministic があれば常に優先する。
- FR-002: source ingest は `source server -> gateway -> daemon` の pull ベースで扱う。
- FR-003: poller source は常時有効で、deterministic stale/down 時の最終 fallback とする。
- FR-004: source health を保持し、`healthy/degraded/down` の判定を行う。
- FR-005: freshness policy は固定値を採用する。
  - fresh: deterministic 最終イベント <= 3s
  - stale: > 3s
  - down: > 15s または transport unhealthy
- FR-006: re-promotion は fresh deterministic 1イベントで即時実施する。
- FR-007: `managed/unmanaged` は agent session 有無で判定する（deterministic 有無では判定しない）。
- FR-008: managed pane の evidence mode は `deterministic` / `heuristic` を別軸で持つ。
- FR-009: 非agent pane（zsh等）は `unmanaged` とし、evidence mode は `none` とする。
- FR-010: daemon client API は `list_panes` / `list_sessions` / `state_changed` / `summary_changed` を提供する。
- FR-011: source health と winner source を可観測化（API/ログ/ストレージ）する。
- FR-012: runtime supervisor は source->gateway->daemon->UI の順に起動し、異常時再起動を行う。
- FR-013: v5 MVP の deterministic source は `Codex appserver` と `Claude hooks` を一次対象として固定する。
- FR-014: 将来の provider capability 追加（例: Codex hooks など）を source server 追加で取り込める拡張設計にする。
- FR-015: pane/session tile title は、agent session と pane session の handshake 完了時に agent session name を最優先で表示する。
- FR-016: title 解決は v4 実装の有効ロジック（canonical session index, binding history）を再利用可能な crate として取り込み、UI間で一貫表示する。
- FR-017: online/e2e source tests（codex/claude）は実行前に `just preflight-online` を必須とし、tmux/CLI auth/network の未準備時は fail-closed で中止する。

## Non-functional Requirements
- NFR-Performance:
  - state update latency p95 < 3s
  - `status` 応答 < 500ms
  - `tmux-status` 応答 < 200ms
- NFR-Reliability:
  - source 障害を局所化し、他コンポーネントの継続動作を保証
  - fallback continuity（deterministic outage 中でも unknown 化しない）
- NFR-Security:
  - ローカル UDS 前提、明示的に許可された bridge/source のみ受理
  - payload 検証、pane_id conflict 検出、dedup を実施
- NFR-Observability:
  - source health、reject reason、winner source、fallback/re-promotion 回数を計測
- NFR-DX:
  - fixture/replay/proptest/accuracy gate を維持
  - local-first 開発ループを標準化し、品質ゲート実行は `just fmt` / `just lint` / `just test` / `just verify` を正とする
  - 日次の開発/検証で commit/PR を必須化しない（git workflow は任意）
  - provider 追加時は source server 追加中心で実装できる
  - v4 の安定ロジック（poller/title/source-health）を再利用して実装速度を維持する

## Constraints
- Tech constraints:
  - Rust workspace 構成を維持し、`agtmux-core` は pure logic を保持
  - test/quality の実行入口は root `justfile` に集約する（内部で strict cargo flags を維持）
  - v4 の provider TOML・fixtures・accuracy gate 定義を再利用する
  - tmux backend は既存機能互換（socket指定/環境変数解決含む）
- Compatibility policy:
  - 原則「最小 fallback」。互換維持は `status/tui/tmux-status` の主要体験を優先。
  - 破壊的変更は ADR + migration note 必須。
- Protocol policy:
  - gateway-daemon 間は JSON-RPC over UDS を採用する。
- UI language policy:
  - `agents` 表記は CLI/TUI/GUI で英語固定とする。

## Open Questions
- Q-001: v5 で poller ベースラインを再測定する際の公式指標セットを何にするか（weighted F1 + waiting系 recall など）？
