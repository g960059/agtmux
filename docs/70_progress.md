# Progress Ledger (append-only)

## Rules
- Append only. 既存履歴は書き換えない。
- 記録対象: 仕様変更、判断、ユーザー要望、学び、gate証跡。

---

## 2026-02-25
### Current objective
- v5 blueprint 用 docs を、テンプレ準拠の構造 (`00`〜`90`) で再編し、v4実装知見を反映する。

### What changed (and why)
- `docs/00_router.md` を作成し、docs-first運用契約を固定。
- `docs/10_foundation.md` と `docs/20_spec.md` を追加し、v5 の安定意図と可変要件を分離。
- 既存 `30/40/50` をテンプレ構造に合わせて再記述し、2層化・外部server・fallbackを実装可能粒度で定義。
- `60/70/80/85/90` を新設し、実行管理・判断記録・レビュー導線を整備。

### Evidence / Gates
- Context evidence:
  - v5 existing docs: `docs/30_architecture.md`, `docs/40_design.md`, `docs/50_plan.md`
  - v3 docs: `docs/v3/*`
  - v4 docs/code: `/Users/virtualmachine/ghq/github.com/g960059/agtmux=v4/docs/v4/*`, `crates/*`
- Tests:
  - 未実行（本作業は docs 更新のみ）
- Typecheck:
  - 未実行
- Lint:
  - 未実行

### Learnings (repo-specific)
- Patterns:
  - v4 は `orchestrator.rs` に priority/fallback/health/dedup が集中。
  - source priority は実装済み（Claude: Hook>File>Poller、Codex: Api>Hook>File>Poller）。
  - source health freshness は `probe_interval + probe_timeout + 250ms` で判定。
- Pitfalls:
  - source ingest と snapshot refresh の同居により、責務境界とテスト境界が曖昧になりやすい。

### Next
- Next action:
  - Open Questions（Q-001〜Q-004）の回答を受けて tasks を確定し、T-010以降へ進む。
- Waiting on user? yes

---

## 2026-02-25
### Current objective
- ユーザー回答を仕様へ反映し、未決を縮小する。

### What changed (and why)
- poller 約85%は「v4時点の体感ベースライン」として再定義し、v5で再測定する方針へ更新。
- v5 MVP deterministic source を `Codex appserver` / `Claude hooks` で固定。
- gateway-daemon protocol を JSON-RPC over UDS で固定。
- `agents` 表記を英語固定で確定。
- 将来 capability 追加に備え、source server 拡張前提を architecture/design/tasks に追記。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー応答で上記4項目を確定
- Tests:
  - 未実行（docs 更新のみ）

### Learnings (repo-specific)
- 明示的な「固定事項」と「将来拡張余地」を分離して記述すると、実装フェーズで迷いが減る。

### Next
- Next action:
  - T-010（v5 crate skeleton）着手
  - T-033（poller baseline 再測定指標）を spec 化
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v4資産の再利用方針を実装計画へ組み込み、pane title 要件を固定する。

### What changed (and why)
- plan/tasks に v4再利用（poller/title/source-health）の明示タスクを追加。
- pane/session handshake 完了時に agent session name を優先表示する仕様を `spec/design` に追加。
- 該当方針を ADR に追記し、MVP固定事項として扱うようにした。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（v4再利用 + handshake title priority）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - T-010/T-011/T-012/T-013 の順で実装着手
- Waiting on user? no

---

## 2026-02-25
### Current objective
- `managed/unmanaged` と `deterministic/heuristic` の語彙混線を解消し、命名規約を固定する。

### What changed (and why)
- `20_spec.md` に 2軸（presence / evidence mode）の命名規約を明示し、5カテゴリの推奨名と表示ラベルを追加。
- `30_architecture.md` の key flow を修正し、presence 判定と handshake による mode 昇格を分離。
- `40_design.md` の統合テスト観点を修正し、「managed化」と「deterministic昇格」を別ケース化。
- ADR に `managed/unmanaged` 固定定義と推奨 naming を追記。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（v4定義との整合、5カテゴリ命名の明確化）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - UI/API フィールド名（presence, evidence_mode）の実装時命名を T-050/T-060 で固定
- Waiting on user? no

---

## 2026-02-25
### Current objective
- Router を docs-first template 準拠に戻し、project固有記述の責務分離を明確化する。

### What changed (and why)
- `00_router.md` を process-only 契約へ再編し、subagent delegation / orchestrator ownership / plan mode policy / NEED_INFO loop を template 構成で明示した。
- `00_router.md` から仕様寄りの記述を排除し、意図・仕様は `10/20+` を正本とするルールを固定した。
- `60_tasks.md` のタイトルを template どおり `Orchestrator only` に更新した（内容は不変）。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（template準拠、Router責務の厳格化、subagent中心運用）
- Tests:
  - 未実行（docs 更新のみ）

### Next
- Next action:
  - `20+` を中心に実装可能粒度の記述を維持し、Routerへの逆流を防止する
- Waiting on user? no

---

## 2026-02-25
### Current objective
- local-first 開発フローを固定し、test/quality コマンドを `just` へ統一する。

### What changed (and why)
- `00_router.md` の Quality Gates を `just fmt` / `just lint` / `just test` / `just verify` に統一し、日次開発で commit/PR 非必須を明記。
- online/e2e source tests（codex/claude）に `just preflight-online` を必須化し、tmux/CLI auth/network 未準備時は fail-closed で中止する運用を追加。
- `20_spec.md` に FR-017 と DX/Constraint を追加し、preflight 要件と `justfile` 一元化を仕様へ昇格。
- `50_plan.md` と `60_tasks.md` を更新し、`justfile` 整備と source別テストスクリプト整備を明示タスク化。
- root `justfile` を新規追加し、`fmt/lint/test/verify/preflight-online/test-source-*` の実行入口を定義。

### Evidence / Gates
- User decision:
  - 2026-02-25 ユーザー要求（git workflow 非依存の local 検証 + `just` 統一）
- Commands:
  - `just --list`（PASS）
- Tests:
  - `just verify` は未実行（workspace 実装前）

### Next
- Next action:
  - T-034 で `scripts/tests/test-source-*.sh` を実装し、preflight付き online/e2e を運用化
- Waiting on user? no

---

## 2026-02-25
### Current objective
- v4を参照した online/e2e source tests を実装し、実行証跡を取得する。

### What changed (and why)
- `justfile` の preflight codex auth check を `codex login status` ベースへ修正し、現行CLI仕様と一致させた。
- `scripts/tests/test-source-codex.sh` / `test-source-claude.sh` / `test-source-poller.sh` を追加し、v4 wait=60（40s running / 120s idle）観測フローを shell で再現。
- claude では workspace trust gate の通過処理を追加し、無人実行で詰まらないようにした。
- test実行workspaceを `/tmp/agtmux-e2e-*` の隔離git repoへ切り替え、このrepoへ provider CLI session が紐づかないようにした。
- cleanup を強化し、各テスト終了時に tmux session/pane child process/temp workspace を自動削除するようにした。
- `60_tasks.md` の T-034 を DONE 化し、観測結果の差分（codexの120s内未確定）を注記した。

### Evidence / Gates
- Commands:
  - `just preflight-online`（PASS）
  - `just test-source-poller`（PASS: t+40s=`sleep`, t+120s=`zsh`）
  - `just test-source-codex`（PARTIAL: capture取得、`wait_result`未観測）
  - `just test-source-claude`（PASS: t+40s running, t+120s `wait_result=idle`）
- Tests:
  - online/e2e の基本実行導線は動作確認済み

### Next
- Next action:
  - codex ケースの prompt/観測窓を調整し、`wait_result`確定までの安定化を行う
- Waiting on user? no

---

## 2026-02-25
### Current objective
- provider model固定（claude/codex）と codex e2e 安定化を完了する。

### What changed (and why)
- claude e2e launch command を `--model claude-sonnet-4-6` 固定へ更新し、capture上で model marker を検証するようにした。
- codex e2e launch を interactive TUI から `codex exec --json`（v4 manifest 準拠）へ変更し、`--model gpt-5.3-codex` + `-c model_reasoning_effort=\"medium\"` を固定。
- codex は 40/120 より安定する 50/180 観測窓へ調整し、running時は pane process (`node/codex`)、idle時は `wait_result=idle` + `turn.completed` で判定するようにした。
- 既存の isolation/cleanup（tmp workspace, tmux session, child process cleanup）は維持。

### Evidence / Gates
- Commands:
  - `just preflight-online`（PASS）
  - `just test-source-codex`（PASS: model/effort marker, running@50s, idle marker@180s）
  - `just test-source-claude`（PASS: Sonnet 4.6 banner, running@40s, idle marker@120s）
- Post-check:
  - `tmux list-sessions | rg agtmux-e2e`（no residual sessions）
  - `/tmp/agtmux-e2e-*`（no residual workspaces）

### Next
- Next action:
  - codex/claude/poller の共通アサーションを script library 化して重複を削減する
- Waiting on user? no

---

## 2026-02-25
### Current objective
- e2e の連続信頼性（各agent 10回）と短縮/並列実行の成立性を確認する。

### What changed (and why)
- codex/claude script を `WAIT_SECONDS=30|60`、`PROMPT_STYLE=strict|compact`、agent別観測窓 override に対応させた。
- codex prompt は揺れ低減のため `wait_result=idle` 固定出力へ変更し、running 判定は pane process で担保する構成へ調整した。
- batch runner `scripts/tests/run-e2e-batch.sh` を追加し、codex/claude の並列反復実行と pass/fail 集計を自動化。
- matrix runner `scripts/tests/run-e2e-matrix.sh` を追加し、異なる時間窓/プロンプト（fast-compact / conservative-strict）を並列実行できるようにした。
- `justfile` に `test-e2e-batch` / `test-e2e-matrix` を追加。

### Evidence / Gates
- Commands:
  - `ITERATIONS=10 WAIT_SECONDS=30 PROMPT_STYLE=compact PARALLEL_AGENTS=1 AGENTS=codex,claude just test-e2e-batch`
    - codex: 10/10 pass
    - claude: 10/10 pass
    - total: 20/20 pass (100%)
  - `ITERATIONS_PER_CASE=2 PARALLEL_CASES=1 just test-e2e-matrix`
    - fast-compact: PASS
    - conservative-strict: PASS
- Post-check:
  - `tmux list-sessions | rg agtmux-e2e`（no residual sessions）
  - `/tmp/agtmux-e2e-(codex|claude|poller)-*`（no residual workspaces）
  - batch/matrix logs は `/tmp/agtmux-e2e-batch-*` / `/tmp/agtmux-e2e-matrix-*` に保持

### Next
- Next action:
  - 10x gate を nightly/手動 gate へ昇格し、失敗時は対応する iteration log を Review Pack に添付する
- Waiting on user? no
