# ADR 2026-02-25: v5 MVP Source Policy and Protocol Locks

## Status
- Accepted

## Context
- v5 docs 初期化後、MVP の deterministic source 範囲、protocol、UI表記、poller品質前提を固定する必要があった。
- v4 実装では poller が有効に機能しており、v5 でも fallback として再利用する前提。

## Decision
- v5 MVP deterministic source は以下で固定する。
  - Codex: appserver
  - Claude: hooks
- gateway-daemon 間 protocol は JSON-RPC over UDS を採用する。
- `agents` 表記は CLI/TUI/GUI で英語固定とする。
- poller 約85%は v4 時点の体感正答率として扱い、v5では公式指標で再測定する。
- 将来 capability（例: Codex hooks）追加は source server 追加で吸収する。
- pane/session handshake 完了時の session tile title は agent session name を優先表示する。
- v4 の poller/title/source-health の安定ロジックは reusable component として再利用する。
- `managed/unmanaged` 判定は agent session の有無で固定し、deterministic/heuristic は evidence mode として別軸管理する。
- naming は `agent_session_{deterministic|heuristic}` / `pane_managed_{deterministic|heuristic}` / `pane_unmanaged_non_agent` を推奨する。

## Consequences
- Positive:
  - MVP スコープが明確化され、実装優先度を切りやすい。
  - protocol/UI 表記の迷いが減る。
  - 将来拡張を daemon 改修最小で扱える。
- Negative / risks:
  - poller baseline が主観起点のため、早期に定量基準化が必要。
  - protocol 固定により将来移行（例: gRPC）時は追加コストが発生。

## Alternatives
- A:
  - MVP から Codex hooks も同時対応する
  - 却下理由: 仕様不確実性が高く、MVP 速度を落とす。
- B:
  - protocol を未固定にして実装段階で決める
  - 却下理由: interface churn で下流設計が不安定化する。

## Links
- Related tasks:
  - `docs/60_tasks.md` T-010, T-033
- Related commits/PRs:
  - docs update only (no code PR yet)
