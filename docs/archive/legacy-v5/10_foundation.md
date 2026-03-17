# Foundation (stable; user approval required to change)

## Overview
- What:
  - AGTMUX v5 を greenfield で再構築し、状態推定を「確定層（deterministic）」と「推定層（heuristic）」の2層に分離する。
  - source ingest を外部 server 化（source servers + gateway）して、daemon は解決器とread model配信に集中させる。
  - `managed/unmanaged` は agent session 有無で定義し、`deterministic/heuristic` とは独立軸として扱う。
- For whom:
  - tmux 上で Claude/Codex/Gemini/Copilot を並行運用する開発者。
- Why now:
  - v4 は機能成立したが、orchestrator 集約設計のためテスト境界と拡張境界が粗く、provider拡張時の変更範囲が大きい。

## Background
- Context:
  - v5 は `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4` からの刷新プロジェクト。
  - v4 では poller を含む multi-source 集約を daemon 内で実施し、source priority/fallback を実装済み。
  - v4 の poller は実運用で有効だったため、v5 では fallback 層として継続活用する。
- Constraints (time/tech/org):
  - v4 運用を継続しながら段階的に v5 を立ち上げる（big-bang cutover 禁止）。
  - 既存 provider 定義（`providers/*.toml`）と fixture 資産（`fixtures/`）を再利用する。
  - v3/v4 の quality gate 指標（accuracy/perf）を下限として維持する。

## Problems / Challenges
- Pain points:
  - source ingest・health・arbitration・UI配信が v4 daemon/orchestrator に集約され、責務分離が弱い。
  - deterministic と poller fallback のテストが同一プロセス依存になり、故障注入テストが重い。
  - provider追加時に daemon 変更が前提となり、拡張コストが高い。
- Risks:
  - 外部プロセス化で運用複雑度は増える。
  - source protocol の揺れを受けると全体が不安定化する。
  - fallback 品質基準が曖昧だと cutover 判定が主観化する。

## Persona
- P1: マルチ Agent パワーユーザー
  - Goals:
    - どの pane が入力/承認待ちかを 3 秒以内に把握したい。
    - 監視のために pane 巡回したくない。
  - Constraints:
    - tmux 常用、複数 agent 並行、運用中断コストが高い。
  - Behaviors:
    - `status`/`tui`/`tmux-status` を併用し、常時監視する。
- P2: ランタイム保守者
  - Goals:
    - source 障害を隔離し、再起動/再接続で復旧可能にしたい。
  - Constraints:
    - 既存 v4 利用者への影響最小化。
  - Behaviors:
    - fixture/replay/CI gate で変更を検証してから段階投入する。

## User Stories (stable IDs)
- US-001: 確定シグナル優先の状態表示
  - As a マルチ agent 利用者
  - I want deterministic source が生きているときは poller 推定に上書きされない
  - So that 誤判定で不要な操作をしない
- US-002: source 障害時の継続監視
  - As a 利用者
  - I want deterministic source が落ちても poller fallback で監視が継続する
  - So that 監視自体が途切れない
- US-003: 既存セッションと新規セッションの一貫動作
  - As a 利用者
  - I want `managed/unmanaged` は agent session 有無で一貫し、deterministic 有無とは分離される
  - So that 状態解釈を誤らない
- US-004: 拡張容易性
  - As a 保守者
  - I want 新 provider を source server 追加で導入できる
  - So that daemon の大規模改修を回避できる
- US-005: 再現可能な品質判定
  - As a 保守者
  - I want replay と gate 指標で cutover 可否を判断できる
  - So that リリース判断を再現可能にできる

## Goals (measurable)
- G-001: state update latency p95 < 3s
- G-002: deterministic source が fresh のとき誤って heuristic winner にならない（回帰テスト 100% pass）
- G-003: poller fallback を常時有効の fallback 層として維持し、deterministic down/stale 時に遷移して監視継続率 100%
- G-004: fallback quality（poller）は v4 時点の体感正答率（約85%）を初期ベースラインとして維持し、v5で計測指標を定義して再検証する
- G-005: source server 障害を他コンポーネントへ伝播させない（個別再起動で復旧可能）

## Non-goals
- NG-001: v4 コードベースのインプレース改修
- NG-002: provider ごとに異なる UI 仕様の導入
- NG-003: 初期フェーズでの分散/リモート multi-node 運用

## Acceptance Criteria
### Global AC
- AC-G-001: architecture/design/plan/task/progress が docs-first で同期されている
- AC-G-002: deterministic > heuristic（tier winner）、fallback、re-promotion の仕様がテストで再現可能
- AC-G-003: v4 poller-only 相当シナリオで観測品質が劣化しない
- AC-G-004: `managed/unmanaged` は agent session 有無で判定され、`deterministic/heuristic` とは独立軸として扱われ、`agents` 表示は managed に対して一貫する

### Per User Story AC
- US-001:
  - AC-001-1: Codex API fresh 時に hook/file/poller event は suppress される
  - AC-001-2: Claude hook fresh 時に poller event は suppress される
- US-002:
  - AC-002-1: deterministic source が stale/down で poller event が受理される
  - AC-002-2: fresh deterministic event 再到達で即座に再昇格する
- US-003:
  - AC-003-1: agent session が確認できる pane は deterministic 未接続でも `managed`（`agents`）として扱う
  - AC-003-2: zsh 等の agent session がない pane は `unmanaged` として扱う
  - AC-003-3: deterministic handshake は presence を変えず、evidence mode（deterministic/heuristic）だけを更新する
- US-004:
  - AC-004-1: source server 追加で gateway/daemon の既存契約を壊さず統合できる
  - AC-004-2: source 障害時に他 source の ingest loop は継続する
- US-005:
  - AC-005-1: replay/fixture/accuracy gate の証跡が Review Pack に添付される
  - AC-005-2: GO/NO_GO 判定が gate 指標と一致する
