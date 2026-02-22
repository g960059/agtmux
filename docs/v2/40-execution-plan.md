# AGTMUX v2 Execution Plan

Date: 2026-02-21
Status: Ready-to-start
Depends on: `./10-product-charter.md`, `./20-unified-design.md`, `./30-detailed-design.md`

## 1. Goal

v2をゼロから実装開始し、MVP到達までの順序とゲートを固定する。

## 2. Phase Plan

## Phase A0: Fork Bootstrap

1. `wezterm-gui fork` 作成（long-lived `fork/main`）
2. `third_party/wezterm` submodule pin 導入
3. `specs/74-fork-surface-map.md` の allowed/restricted チェックを CI 追加
4. `scripts/ci/check-submodule-window.sh` / `scripts/ci/check-fork-surface.sh` の雛形作成

Gate:

1. fork/main build pass
2. restricted zone change check pass

## Phase A1: Fork Hook Map Spike

1. `specs/75-fork-hook-map-spike.md` の成果物を作成
2. file/function-level hook points を確定
3. Phase C 実装範囲を hook map に固定

Gate:

1. hook map sufficient 判定 pass
2. Phase C で追加調査不要な状態を確認

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
2. `AgtmuxRuntimeBridge` / `TerminalFeedRouter` 実装
3. attach/focus/write/resize
4. IME preedit/commit

Gate:

1. cursor/scroll/IME replay pass
2. `71-quality-gates.md` の **Dev Gate** を満たす
3. restricted zone modifications = 0 (or ADR attached)

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

1. `71-quality-gates.md` の **Beta Gate**（state/attention）を満たす

## Phase G: Multi-target Hardening

1. ssh pooling/backoff
2. target isolation
3. runbook + release checklist

Gate:

1. local unaffected under ssh failure
2. `71-quality-gates.md` の **Release Gate** を満たす

## Phase H: Hotpath Framing Decision

1. `specs/76-output-hotpath-framing-policy.md` に従い計測
2. MessagePack 維持 or `output_raw` 導入を ADR で決定
3. 導入時は protocol spec 更新と回帰計測を実施

Gate:

1. decision ADR merged
2. SLO 達成を再確認

## 3. Bootstrap Commands

```bash
cd /Users/virtualmachine/ghq/github.com/g960059/agtmux

cargo init --vcs none .
mkdir -p crates apps scripts/ui-feedback

cargo new --lib crates/agtmux-protocol
cargo new --lib crates/agtmux-target
cargo new --lib crates/agtmux-tmux
cargo new --lib crates/agtmux-state
cargo new --lib crates/agtmux-agent-adapters
cargo new --lib crates/agtmux-store
cargo new --bin crates/agtmux-daemon
cargo new --bin crates/agtmux-cli
cargo new --bin apps/desktop-launcher
git submodule add <YOUR_WEZTERM_FORK_URL> third_party/wezterm
git -C third_party/wezterm checkout <PINNED_COMMIT_OR_TAG>
```

レイアウトの正本は `./specs/72-bootstrap-workspace.md` を参照。

## 4. Cross-cutting UI Feedback Loop

UIを触るphaseでは、下記を phase gate 前に実行する。

1. `AGTMUX_RUN_UI_TESTS=1 ./scripts/ui-feedback/run-ui-tests.sh`
2. `./scripts/ui-feedback/run-ui-feedback-report.sh 1`
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
4. `specs/74-fork-surface-map.md` の allowed/restricted を確認
5. `specs/75-fork-hook-map-spike.md` の要件を確認（Phase C着手前）

Implementation:

1. protocol -> stream -> desktop -> UX -> state の順序を守る
2. active path に snapshot を入れない
3. stale epoch を fail-closed にする

Validation:

1. replay tests（codex/claude/gemini/CJK/IME）
2. layout rollback tests
3. ssh partial-result tests
4. UI feedback report (`tests_failures = 0`)
5. phase 対応 gate (`./specs/71-quality-gates.md`) 達成

Release:

1. docs更新
2. runbook更新
3. rollback手順確認
