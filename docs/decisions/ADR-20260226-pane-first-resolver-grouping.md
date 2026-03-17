# ADR 20260226: Pane-First Resolver Grouping

## Status
- Accepted

## Context
- ライブテストで deterministic evidence (Codex AppServer) が heuristic (Poller) に上書きされる現象を発見。
- 根本原因: `apply_events()` が `session_key` でイベントをグループ化するが、各 source は異なる `session_key` を使用する。
  - Poller: `"poller-{pane_id}"` (Heuristic tier)
  - Codex AppServer: `thread_id` (Deterministic tier)
  - Claude Hooks: `session_id` (Deterministic tier)
- 結果: 同一 pane のイベントが別々の resolver batch で処理され、`project_pane()` の last-writer-wins で Heuristic が Deterministic を上書きできる。
- 再現テスト 9 件で 3 件 FAIL として bug を確認。

## Decision
- `apply_events()` のグループ化キーを `session_key` → `pane_id` に変更する。
  - Fallback: `event.pane_id` → `session_to_pane.get(session_key)` → `session_key`
- `resolver_states` のキーも同様に `pane_id` (or fallback) で管理する。
- 1 グループに複数 `session_key` がある場合、resolver 出力を各 session に個別投影する。
- `project_pane()` の `deterministic_fresh_active` 参照先も pane の group_key の resolver state に変更する。
- **核心不変条件: 同一 pane の全ソースイベントが同一 resolver batch で処理される。**

## Consequences
- Positive:
  - 同一 pane の cross-source tier 抑制が正しく機能する。
  - resolver.rs は pure function のまま変更不要 — グループ化は呼び出し側の責務。
  - Provider 切り替え時 (Codex→Claude) は 3s freshness window で自然に切り替わる。
  - 変更対象は `projection.rs` のみ (4 modification points)。
- Negative / risks:
  - `pane_id` が None のイベントは `session_to_pane` fallback に依存する。

## Alternatives
- A: `session_key` でグループ化のまま、`PaneTierArbiter` で二段解決
  - 却下理由: Codex→Claude 切り替え時に Codex AppServer が thread/list events を出し続け `det_last_seen` を fresh に保つため、Claude heuristic が永久にブロックされる。
- B: 各 source の `session_key` を統一フォーマットに変更
  - 却下理由: `session_key` は dedup にも使われるため、source 固有の粒度を維持する必要がある。

## Links
- Related docs:
  - `docs/20_spec.md` (FR-031a)
  - `docs/40_design.md` (Section 3: Resolver and Arbitration)
  - `docs/80_decisions/ADR-20260225-pane-signature-v1.md`
- Related tasks:
  - `docs/60_tasks.md` T-121
- Tests:
  - `crates/agtmux-daemon-v5/src/projection.rs` — 9 cross-session tests (#37-#45)
