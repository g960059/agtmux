# AGTMUX v2 Execution Plan

Date: 2026-02-21
Status: Ready-to-start
Depends on: `./10-product-charter.md`, `./20-unified-design.md`, `./30-detailed-design.md`

## 1. Goal

v2をゼロから実装開始し、MVP到達までの順序とゲートを固定する。

## 2. Phase Plan

## Phase A: Foundation

1. Rust workspace作成
2. `agtmux-protocol` 実装
3. `agtmux-store` migration実装

Gate:

1. codec roundtrip test pass
2. migration repeatability pass

## Phase B: Stream Core

1. pane tap manager
2. selected pane stream-only enforcement
3. control bridge fallback

Gate:

1. selected hotpath capture count = 0
2. pane open local p95 < 150ms

## Phase C: Desktop Terminal

1. `wezterm-gui fork` 統合
2. attach/focus/write/resize
3. IME preedit/commit

Gate:

1. cursor/scroll/IME replay pass
2. local input p95 < 25ms

## Phase D: Sidebar + Window UX

1. filter tabs (`all|managed|attention|pinned`)
2. organize menu
3. pane/session context menu
4. window-grouped mode

Gate:

1. no reorder jitter
2. `Open Pane` / `Open tmux Window` E2E pass

## Phase E: Layout Mutation

1. DnD drop targets
2. `layout_mutate` + lock + rollback
3. mutation conflict tests

Gate:

1. topology divergence = 0

## Phase F: State + Attention

1. adapter registry
2. codex/claude/gemini adapters
3. attention queue

Gate:

1. status precision gate pass
2. attention precision gate pass

## Phase G: Multi-target Hardening

1. ssh pooling/backoff
2. target isolation
3. runbook + release checklist

Gate:

1. local unaffected under ssh failure
2. perf gate pass

## 3. Bootstrap Commands

```bash
mkdir -p agtmux-rs/{crates,apps,docs}
cd agtmux-rs
cargo new --lib crates/agtmux-protocol
cargo new --lib crates/agtmux-target
cargo new --lib crates/agtmux-tmux
cargo new --lib crates/agtmux-state
cargo new --lib crates/agtmux-agent-adapters
cargo new --lib crates/agtmux-store
cargo new --bin crates/agtmux-daemon
cargo new --bin crates/agtmux-cli
cargo new --bin apps/agtmux-desktop
```

## 4. Cross-cutting UI Feedback Loop

UIを触るphaseでは、下記を phase gate 前に実行する。

1. `cd macapp && AGTMUX_RUN_UI_TESTS=1 ./scripts/run-ui-tests.sh`
2. `./scripts/run-ui-feedback-report.sh 1`
3. report の `tests_failures = 0` を確認
4. report artifact を implementation record に添付

補足:

1. SSH経由実行は禁止（TCC適用外）
2. GUIログインセッションの Terminal.app / Xcode で実行する
3. AX不安定による skip は許容するが、理由を明記する

詳細運用は `./60-ui-feedback-loop.md` を参照。

## 5. Execution Checklist

Preflight:

1. `10-product-charter.md` を確認
2. `20-unified-design.md` の fork一本方針を確認
3. `30-detailed-design.md` の invariants を確認

Implementation:

1. protocol -> stream -> desktop -> UX -> state の順序を守る
2. active path に snapshot を入れない
3. stale epoch を fail-closed にする

Validation:

1. replay tests（codex/claude/gemini/CJK/IME）
2. layout rollback tests
3. ssh partial-result tests
4. UI feedback report (`tests_failures = 0`)

Release:

1. docs更新
2. runbook更新
3. rollback手順確認
