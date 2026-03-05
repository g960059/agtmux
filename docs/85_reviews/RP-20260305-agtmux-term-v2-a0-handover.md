# Review Pack: agtmux-term V2 A0 Handover (for Orchestrator)

Date: 2026-03-05
Owner: agtmux Orchestrator
Policy: No backward compatibility

## Context
`agtmux-term` 側で observed されている UX 問題（初回表示遅延、sidebar一時空白、metadata timeout時の情報欠落）は、
term側単独の回避では限界がある。

最短で効くのは daemon/runtime 側の「fast cached snapshot」契約の明文化と実装。

## Adopted Direction
1. inventory is canonical existence
2. metadata is non-destructive overlay
3. client must render inventory without waiting metadata

## Required daemon/runtime changes (A0)
1. Cached snapshot read path
- `json` 応答で、最新inventoryとlast-known metadataを即返却可能にする。
- metadata収集が遅延/失敗しても、inventory rows は保持する。

2. Non-destructive metadata failure semantics
- metadata subsystem timeout/error で pane rows を空化しない。
- stale metadataは stale として保持し、上書き可能な状態で返す。

3. Failure budget / backoff contract
- metadata fetch failure時の retry/backoff 方針を固定。
- term側が過剰再試行しなくてよいよう、daemon側で吸収する。

## Contract to term (A0)
- termは inventory-first で表示。
- daemon metadata失敗時も row existence は不変。
- metadataは遅延到着でも overlay可能。

## Out of scope (A0)
- protocol version negotiation
- seq/epoch delta replay strictization
- ack compaction / true streaming
- pane_instance_id generation extension

## Acceptance Criteria (cross-repo)
1. metadata timeout時にも local panes が消えない
2. first local rows p95 <= 400ms
3. metadata enrichment p95 <= 1.5s
4. same-window pane switch p95 <= 0.5s

## Handover Checklist
- [ ] daemon cached snapshot path実装
- [ ] metadata failure semanticsをnon-destructive化
- [ ] termでinventory-first lane分離実装
- [ ] cross-repoで上記AC測定

## Notes
- A1/A2（protocol hardening/optimization）はA0 UX改善を確認後に着手する。
