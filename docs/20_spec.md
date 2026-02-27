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

## Identity Model (pane-first)
- Canonical runtime identity:
  - `pane_instance = (pane_id, generation, birth_ts)`
  - `pane_id` は tmux 再利用で衝突し得るため、単独では実体キーにしない。
- Session link semantics:
  - `session_key` は provider 会話スレッド識別子として扱う（fork/resume で変化し得る）。
  - pane と session は 1:1 固定にしない。pane-first で時系列 link として管理する。
- Binding churn policy:
  - 同一 `pane_id` の再出現時は `generation` 更新で新実体化し、旧実体は grace 期間 `tombstone` 扱いにする。
  - grace window は `120s` 固定。grace 中は遅延イベントの参照先として tombstone を維持する。
  - tombstone の保持期限は `24h` 固定。`24h` 経過かつ遅延イベント未到達で purge する。
  - `pane_id` 再利用時は必ず generation を単調増加させ、旧 generation を再活性化しない。

## Pane Signature Model (v1)
- Signature classes:
  - `deterministic`: hooks/appserver 由来の handshake/event で pane と agent session を結び付けられる
  - `heuristic`: poller/process/cmd/title 由来の推定
  - `none`: non-agent または判定不能
- Deterministic signature minimum fields:
  - `provider`, `source_kind`, `pane_instance_id`, `session_key`, `source_event_id`, `event_time`
  - optional: `runtime_id`, `agent_session_name`
- Heuristic signals (high -> low):
  - process/provider hint（tmux + ps の pid/tty/args 推定）
  - `current_cmd` token match
  - poller capture signal（tail出力）
  - `pane_title` token match
- Heuristic hysteresis:
  - idle 確定は `max(4s, 2 * poll_interval)` の安定観測後
  - managed pane の poller running 昇格は「running hintあり」かつ `last_interaction <= 8s` のときのみ
  - managed pane の poller running は running hint消失後 `45s` 超で idle へ降格
  - heuristic `no-agent` は連続2回観測で `unmanaged` へ降格（deterministic 有効時は降格しない）
- Guardrails:
  - title-only match は単独で managed 昇格根拠にしない
  - wrapper command（`node`/`bun`/`deno`）かつ provider hint 不在で title-only の場合は無効化する

## Execution Scope Policy (Mode B)
- Phase 1-2 の実装ブロッカーは `[MVP]` 要件のみ。
- `[Post-MVP]` は設計資産として維持し、Phase 1-2 の未実装を許容する。
- `[Post-MVP]` を前倒し実装する場合は、`docs/60_tasks.md` に依存関係つきで明示する。

## Functional Requirements
- FR-001 `[MVP]`: resolver は tier winner を採用し、fresh deterministic があれば常に優先する。
- FR-002 `[MVP]`: source ingest は `source server -> gateway -> daemon` の pull ベースで扱う。
- FR-003 `[MVP]`: poller source は常時有効で、deterministic stale/down 時の最終 fallback とする。
- FR-004 `[MVP]`: source health を保持し、`healthy/degraded/down` の判定を行う。
- FR-005 `[MVP]`: freshness policy は固定値を採用する。
  - fresh: deterministic 最終イベント <= 3s
  - stale: > 3s
  - down: > 15s または transport unhealthy
- FR-006 `[MVP]`: re-promotion は fresh deterministic 1イベントで即時実施する。
- FR-007 `[MVP]`: `managed/unmanaged` は agent session 有無で判定する（deterministic 有無では判定しない）。
- FR-008 `[MVP]`: managed pane の evidence mode は `deterministic` / `heuristic` を別軸で持つ。
- FR-009 `[MVP]`: 非agent pane（zsh等）は `unmanaged` とし、evidence mode は `none` とする。
- FR-010 `[MVP]`: daemon client API は `list_panes` / `list_sessions` / `state_changed` / `summary_changed` を提供する。
- FR-011 `[MVP]`: source health と winner source を可観測化（API/ログ/ストレージ）する。
- FR-012 `[MVP]`: runtime supervisor は source->gateway->daemon->UI の順に起動し、異常時再起動を行う。
- FR-013 `[MVP]`: v5 MVP の deterministic source は `Codex appserver`、`Claude hooks`、`Claude JSONL` を対象として固定する。Claude JSONL は hooks 未登録環境でも deterministic evidence を提供する。
- FR-014 `[MVP]`: 将来の provider capability 追加（例: Codex hooks など）を source server 追加で取り込める拡張設計にする。
- FR-015 `[MVP]`: pane/session tile title は、agent session と pane session の handshake 完了時に agent session name を最優先で表示する。
- FR-016 `[MVP]`: title 解決は v4 実装の有効ロジック（canonical session index, binding history）を再利用可能な crate として取り込み、UI間で一貫表示する。
- FR-017 `[MVP]`: online/e2e source tests（codex/claude）は実行前に `just preflight-online` を必須とし、tmux/CLI auth/network の未準備時は fail-closed で中止する。
- FR-018 `[Post-MVP]`: gateway は source cursor を `fetched_cursor` / `committed_cursor` の二水位で管理し、`committed_cursor` は daemon の反映完了 ack 後にのみ前進させる。
- FR-019 `[Post-MVP]`: source->gateway 取り込みは at-least-once を前提とし、重複排除キーは `provider + session_key + event_id` で固定する。
- FR-020 `[Post-MVP]`: source が `invalid_cursor` を返した場合は safe rewind（dedup 窓内）で自動復旧し、復旧不能時のみ欠落警告付き再同期へフォールバックする。
- FR-021 `[MVP]`: binding state machine は pane-first（`pane_instance` 基準）で実装し、pane再生成/再接続/移動を遷移で扱う。
- FR-022 `[Post-MVP]`: 同一 session に複数 pane が紐づく場合、session tile の代表 pane は「最新 deterministic handshake 時刻」を優先し、同点は最新 activity で決定する。
- FR-023 `[Post-MVP]`: 遅延SLOは deterministic 経路で `state_changed p95 <= 2.0s`、fallback 劣化時は `p95 <= 5.0s` を採用する。
- FR-024 `[MVP]`: pane 判定は `signature_class`（`deterministic` / `heuristic` / `none`）と `signature_reason` を必須保持し、list API と push event へ露出する。
- FR-025 `[MVP]`: deterministic signature は `provider + source_kind + pane_instance_id + session_key + source_event_id + event_time` を最小契約とする。
- FR-026 `[MVP]`: heuristic signature は v1 優先順位（process_hint > cmd > poller > title）で判定し、重みは `1.00 / 0.86 / 0.78 / 0.66` を既定とする。
- FR-027 `[MVP]`: title-only guard を適用し、title_match のみ（process_hint/cmd_match/capture_match いずれも false）の場合は `current_cmd` に関係なく managed 昇格させない。pane_title は stale になりやすいため、単独シグナルとしては信頼できない。
- FR-028 `[MVP]`: heuristic `no-agent` は連続2観測で `unmanaged` へ降格する。deterministic signature が fresh の間は `managed` を維持する。
- FR-029 `[MVP]`: poller由来の状態補正は `8s` running昇格窓、`45s` running降格窓、`max(4s,2*poll_interval)` idle安定窓を採用する。
- FR-030 `[MVP]`: source rank は provider別に固定し、MVPでは `Codex: appserver > poller`、`Claude: hooks > jsonl > poller` とする（将来 source 追加時は rank を拡張）。
- FR-031 `[MVP]`: `managed/unmanaged` 判定に env 変数の存在を必須条件として使わない（補助シグナルとしてのみ許可）。
- FR-031a `[MVP]`: daemon の `apply_events()` はイベントを `pane_id` でグループ化し（fallback: `session_to_pane` → `session_key`）、同一 pane の全ソースイベントが同一 resolver batch で処理されることを保証する。`session_key` 単位のグループ化は禁止する（異なる source が異なる `session_key` を使うため、cross-source tier 抑制が機能しない）。
- FR-032 `[MVP]`: poller fallback の受入基準は固定し、リリース可否は `weighted F1 >= 0.85` かつ `waiting recall >= 0.85` を必須とする。
- FR-033 `[MVP]`: poller fallback の評価データセットは固定 fixture（`>= 300` labeled windows, Codex/Claude混在）を使用し、指標は毎回同一セットで再計測する。
- FR-034 `[Post-MVP]`: `invalid_cursor` 復旧は数値契約を固定し、checkpoint は `30s` または `500 events` ごと、safe rewind 上限は `10m` または `10,000 events` とする。
- FR-035 `[Post-MVP]`: dedup 保持期間は `rewind_window + 120s` 以上（MVP既定 `>= 12m`）とし、`invalid_cursor` が `60s` 内に3回連続発生した場合は full resync + warning を強制する。
- FR-036 `[Post-MVP]`: UDS 接続は peer credential 検証を必須化し、`peer_uid == runtime_uid` かつ source registry に登録済み（`source_kind + socket_path + owner_uid` 一致）の接続のみ受理する。
- FR-037 `[Post-MVP]`: 遅延SLO判定は rolling `10m` window（サンプル数 `>= 200`）で評価し、budget超過が3連続 window 発生した source は `degraded` へ遷移させる。
- FR-038 `[Post-MVP]`: 永続状態（`session_state_v2`, `pane_state_v2`, `binding_link_v2`, `cursor_state_v2`）は `15m` 間隔+shutdown時に snapshot を取得し、canary 前に restore dry-run を必須化する。
- FR-039 `[Post-MVP]`: supervisor の health 判定は `liveness` と `readiness` を分離し、readiness には依存先到達性（source->gateway->daemon）を含める。
- FR-040 `[Post-MVP]`: supervisor の再起動ポリシーは指数バックオフ（初回 `1s`, 係数 `2.0`, 上限 `30s`, jitter `+-20%`）を採用し、`10m` 内 `5` 回失敗で `hold_down(5m)` + `escalate` を発火する。
- FR-041 `[Post-MVP]`: gateway の delivery 契約は `delivery_token` 単位の冪等処理とし、ack timeout（既定 `2s`）時は同一 token で再配送する。
- FR-042 `[Post-MVP]`: `gateway.ack_delivery` は冪等でなければならず、重複 ack は `already_committed` を返し、`committed_cursor` を再前進させない。
- FR-043 `[Post-MVP]`: source registry は lifecycle（`pending`/`active`/`stale`/`revoked`）を持ち、接続受理は `active` source のみ許可する。
- FR-044 `[Post-MVP]`: source registry の遷移は `source.hello` / heartbeat timeout（既定 `30s`）/ operator revoke で管理し、socket path rotation は同一 `source_kind + owner_uid` で明示再登録を必須とする。
- FR-045 `[Post-MVP]`: binding projection は single-writer（daemon projection loop）で直列化し、`state_version` による CAS 更新で古いイベントの上書きを禁止する。
- FR-046 `[Post-MVP]`: ops guardrail manager は `warn/degraded/escalate` の3段階アラートを生成し、全アラートを `list_alerts` とログへ同時出力する。
- FR-047 `[Post-MVP]`: ack 再配送の試行回数は `max_attempts=5`（既定）とし、上限到達時は source を `degraded` に遷移し `ack_retry_exhausted` を記録する。

## Non-functional Requirements
- Phase gate:
  - `[MVP]` FR に直接紐づく NFR だけを Phase 1-2 の必須ゲートとする。
  - `[Post-MVP]` 向け NFR は設計保持しつつ、Phase 3+ で順次実装する。
- NFR-Performance:
  - deterministic path state update latency (`state_changed`) p95 <= 2.0s
  - fallback degraded path latency p95 <= 5.0s
  - hop budget (p95): source capture 0.6s / gateway ingest 0.4s / daemon merge+notify 0.6s / client reflect 0.4s
  - `status` 応答 < 500ms
  - `tmux-status` 応答 < 200ms
- NFR-Reliability:
  - source 障害を局所化し、他コンポーネントの継続動作を保証
  - fallback continuity（deterministic outage 中でも unknown 化しない）
  - `[Post-MVP]` `invalid_cursor` 連鎖時でも rewind/full-resync のどちらかで最終的に再同期可能であること
  - `[Post-MVP]` snapshot/restore 手順を runbook 化し、canary 前 dry-run 証跡を残すこと
  - `[Post-MVP]` supervisor hold-down 中でも他 healthy source の監視は継続すること
- NFR-Security:
  - ローカル UDS 前提、明示的に許可された bridge/source のみ受理
  - `[Post-MVP]` UDS socket path は runtime 専用ディレクトリ（`0700`）配下、socket file は `0600` を必須とする
  - `[Post-MVP]` peer credential（Linux: `SO_PEERCRED`, macOS: `getpeereid`）で UID 一致を検証する
  - `[Post-MVP]` source registry 未登録接続・runtime nonce 不一致接続は fail-closed で拒否する
  - payload 検証、pane_id conflict 検出、dedup を実施
- NFR-Observability:
  - source health、reject reason、winner source、fallback/re-promotion 回数を計測
  - hop別遅延ヒストグラム、cursor rewind 回数、duplicate/drop 件数、binding churn 率を計測
  - signature class 分布、signature reason 上位、managed/unmanaged flap 回数を計測
  - `[Post-MVP]` SLO breach は rolling window 単位で判定し、warn/degraded/escalate を段階的に発火する
  - `[Post-MVP]` ack retry 回数、ack timeout 回数、`ack_retry_exhausted` 件数を source ごとに計測する
  - `[Post-MVP]` source registry lifecycle 遷移（pending/active/stale/revoked）を監査ログとして保存する
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
- MVP runtime policy:
  - MVP runtime は single-process binary（multi-process extraction は Post-MVP）
  - tmux integration は sync subprocess calls を `spawn_blocking` で wrap して使用
  - State は MVP で in-memory only（SQLite persistence は Post-MVP）
  - UDS socket directory は mode `0700` + per-UID path isolation を MVP から適用
- Compatibility policy:
  - 原則「最小 fallback」。互換維持は `status/tui/tmux-status` の主要体験を優先。
  - 破壊的変更は ADR + migration note 必須。
- Protocol policy:
  - gateway-daemon 間は JSON-RPC over UDS を採用する。
  - `[Post-MVP]` cursor commit は ack ベースで進める（受信時即commitは禁止）。
  - `[Post-MVP]` delivery/ack は `delivery_token` の冪等契約に従い、重複配送を許容する。
- UI language policy:
  - `agents` 表記は CLI/TUI/GUI で英語固定とする。

## Open Questions
- (none)
