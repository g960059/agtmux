# ADR 20260225: Pane Signature v1 (Managed/Unmanaged Detection Contract)

## Status
- Accepted

## Context
- レビューで「managed pane 検出条件が曖昧」という指摘を受けた。
- v4 と go-codex POC を調査すると、判定は env 固定ではなく `event/cmd/process/capture` の複合で実装されていた。
- v5 では pane-first を維持しつつ、実装者依存の解釈を排除するため、signature 契約を仕様として固定する必要がある。

## Decision
- Pane signature を `signature_class + signature_reason + signature_confidence` で管理する。
  - `deterministic`: handshake/event 起点
  - `heuristic`: process/cmd/poller/title 起点
  - `none`: non-agent または判定不能
- Deterministic signature 最小契約:
  - `provider`, `source_kind`, `pane_instance_id`, `session_key`, `source_event_id`, `event_time`
  - optional: `runtime_id`, `agent_session_name`
- Heuristic priority/weights:
  - process/provider hint: `1.00`
  - current_cmd token: `0.86`
  - poller capture signal: `0.78`
  - pane_title token: `0.66`
- Guardrails:
  - title-only match は managed 昇格根拠にしない
  - wrapper command（`node|bun|deno`）かつ provider hint 不在の title-only は reject
- Hysteresis:
  - idle確定: `max(4s, 2*poll_interval)`
  - running昇格: `last_interaction <= 8s` かつ running hint あり
  - running降格: running hint 消失かつ `last_interaction > 45s`
  - heuristic `no-agent` は連続2観測で `none/unmanaged`
- Presence exposure:
  - `signature_class`, `signature_reason`, `signature_confidence`, `signature_inputs` を list/push API に露出する

## Consequences
- Positive:
  - managed/unmanaged 判定が再現可能になり、実装者の裁量が減る。
  - v4 由来の誤判定ガードと POC 由来のフラップ抑制を同時に取り込める。
  - provider source 追加時も signature 契約を維持したまま拡張できる。
- Negative / risks:
  - classifier と hysteresis の状態管理が増え、実装複雑度が上がる。
  - 閾値が環境依存でずれる可能性があり、運用中の再調整余地を残す必要がある。

## Alternatives
- A: env 変数ベースで managed を固定判定
  - 却下理由: v4/POC の実装実態と一致せず、CLI差分に弱い。
- B: title/cmd の単純判定のみ
  - 却下理由: wrapper 実行時の誤検出を抑えきれない。
- C: deterministic のみで managed 判定
  - 却下理由: deterministic 障害時に pane-first 観測の継続性が失われる。

## Links
- Related docs:
  - `docs/20_spec.md` (Pane Signature Model, FR-024〜FR-031)
  - `docs/30_architecture.md` (Flow-002, Flow-008)
  - `docs/40_design.md` (Pane Signature Classifier)
- Related tasks:
  - `docs/60_tasks.md` T-044, T-045, T-046
- Investigation references:
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4`
  - `/Users/virtualmachine/ghq/github.com/g960059/agtmux/.worktrees/exp/go-codex-implementation-poc`
