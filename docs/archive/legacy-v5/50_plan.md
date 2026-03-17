# Plan (mutable; keep it operational)

## Execution Policy (Mode B)
- Phase 1-2 は `[MVP]` 要件だけを実装ブロッカーとする。
- `[Post-MVP]` 要件は Phase 3+ の hardening backlog として維持する。
- 実装中に `[Post-MVP]` が必要と判明した場合のみ、タスクを昇格して着手する。

## Phase 0: Setup / Spec Freeze
- Deliverables:
  - `00/10/20/30/40/50/60/70/80/85/90` の整備
  - FR の `[MVP]` / `[Post-MVP]` タグ付け
  - `Main (MVP Slice)` / `Appendix (Post-MVP)` の設計分離
  - root `justfile`（`fmt` / `lint` / `test` / `verify` / `preflight-online` / `test-source-*`）整備
- Exit criteria:
  - Phase 1-2 実装に必要な仕様が `MVP` スライスだけで完結
  - Post-MVP が非ブロッカーであることを tasks/plan に明記

## Phase 1: Core MVP (types + resolver)
- Deliverables:
  - `agtmux-core-v5`（EvidenceTier, PanePresence, EvidenceMode, SourceEventV2）
  - tier winner resolver
  - pane signature classifier v1
  - v4 再利用ロジック抽出（poller core / source-health / title resolver）
  - fresh/stale/down/re-promotion unit tests
- Exit criteria:
  - deterministic priority と fallback/re-promotion が unit/replay で PASS
  - signature classifier（weights/guard/hysteresis）が PASS
  - related stories: US-001, US-002

## Phase 2: MVP Runtime Path (sources + gateway + daemon + runtime)
- Deliverables:
  - `agtmux-source-codex-appserver`
  - `agtmux-source-claude-hooks`
  - `agtmux-source-poller`
  - gateway basic pull aggregation（single committed cursor）
  - daemon projection + client API (`list_panes/list_sessions/state_changed/summary_changed`)
  - pane-first binding basic flow + handshake title priority
  - Cursor contract fix（source は caught up 時も `Some(cursor)` を返す）
  - `agtmux-tmux-v5` crate（tmux IO boundary + pane generation tracking）
  - `agtmux-runtime` binary crate（CLI + daemon + UDS server）
  - Poll loop wiring: tmux -> poller -> gateway -> daemon（unmanaged pane tracking + compaction 付き）
- Exit criteria:
  - provider priority/suppress/fallback integration tests PASS
  - codex/claude online tests は `just preflight-online` 後に PASS
  - poller fallback quality gate (`weighted F1>=0.85`, `waiting recall>=0.85`) PASS
  - `agtmux daemon` starts, polls tmux, populates projection（managed + unmanaged panes）
  - `agtmux status` connects via UDS and displays pane/session info
  - `just verify` passes with all 8 crates
  - related stories: US-001, US-002, US-003, US-004

## Phase 3: Post-MVP Hardening — Pure-logic crate wiring ✅ COMPLETE
- Deliverables (all wired into runtime):
  - T-118: LatencyWindow → poll_tick SLO evaluation + `latency_status` API + path escaping fix
  - T-116: CursorWatermarks + InvalidCursorTracker → gateway cursor pipeline (advance_fetched/commit + recovery)
  - T-117: SourceRegistry → `source.hello`/`source.heartbeat`/`list_source_registry` + staleness check
  - T-115: TrustGuard → UDS admission gate (warn-only) + `daemon.info` + source.ingest schema extension
- Implementation order: T-118 → T-116 → T-117 → T-115 ("observability first" + "lifecycle before admission")
- Codex plan review: Go with changes (5 findings, all adopted — see 70_progress.md)
- Exit criteria:
  - `just verify` PASS (585 tests = 565 MVP + 20 Phase 3)
  - 4 pure-logic crates (66 existing tests) wired into runtime with 20 integration tests
  - TrustGuard warn-only (enforce deferred to Phase 4)

## Phase 3b: Codex App Server 実働線 ✅ COMPLETE
- Goal: Codex App Server → CLI の deterministic pane 表示を end-to-end で動作させる
- Implementation order: T-120 → T-119
- Deliverables:
  - T-120: Protocol fix + reliability (jsonrpc compliance, reconnection, mutex fix, health, dead code cleanup)
  - T-119: pane_id correlation (thread.cwd ↔ tmux pane cwd → pane-level deterministic detection)
- Exit criteria:
  - `codex app-server` を起動中に `agtmux list-panes` で Codex pane が `signature_class: deterministic` と表示される
  - App Server プロセス kill 後、backoff 再接続で自動復旧する
  - `just verify` PASS
  - `just test-source-codex` で App Server 経由の evidence flow が確認できる

## Phase 4: Hardening Wave 2 — Supervisor strict only
- 実施する:
  - T-129: Supervisor strict wiring — `SupervisorTracker` を codex_appserver 再接続ループに結線（純ロジック crate は実装済み、poll_loop.rs への wiring のみ）
- 実施しない（理由と共に記録）:
  - ~~TrustGuard enforce~~ → **DROPPED**: 個人利用 + 単一ユーザー環境では warn-only で十分。複数ユーザー環境のニーズが生じた時点で昇格
  - ~~Persistence (SQLite)~~ → **DROPPED**: daemon の自然回復は 2〜4 秒（1〜2 poll tick）。tmux の pane_id は tmux server 再起動で変わるため長期保存データは有害になりうる
  - ~~Multi-process extraction~~ → **DROPPED**: GUI バンドル版は single-process で十分。分離のニーズが生じた時点で検討
  - ~~ops guardrail manager / list_alerts~~ → **DROPPED**: 運用規模が小さい間は不要

## Phase 5: Migration / Cutover — DROPPED
- **理由**: v4 は production に進んでおらず、ユーザーもいない。切り替え戦略は不要。
- ~~v4/v5 side-by-side runbook~~
- ~~canary plan + rollback plan~~
- ~~backup/restore runbook~~

## Phase 6: CLI / TUI (新規)
- Goal: daemon のデータを実際に使える形で提供する。tcmux の精密版として位置づけ。
- Design principles (2026-02-28 更新):
  - `list-panes` = サイドバー相当のフラット一覧。session ラベルあり、ペイン単位。
    - agent pane: `provider  title  relative_time` (title = conversation title、T-135 まで provider 名 placeholder)
    - unmanaged pane: `current_cmd`
    - det = 無印（期待値）。heur = `~` prefix（不確かさを明示）
    - `--json`: 旧 JSON 出力 (後方互換)。
    - `--context=auto|off|full`:
      - `auto` (default): `cwd`/`branch` はフィールド単位で header 集約し、pane 行は差分フィールドのみ suffix 表示
      - `off`: `cwd`/`branch` と `mixed` marker を非表示（最小情報）
      - `full`: 行数を増やさず、既存行へ inline で `cwd`/`branch` を常時表示
    - `--path` / `-p` は提供しない（`--context=full` に統一、入力時は `hint: use --context=full` エラー）
    - `-p` は list 系で未割り当て固定（後方互換不要のため、ここで short flag 意味を凍結し表記ゆれを防ぐ）
    - `--summary`: pane summary を opt-in 表示（agent 明示データのみ）
  - `list-windows` = window 単位の集計。@N 非表示 (window_name のみ)。1行/window。
    - `session  window_name  [~] status  count`
    - default `--context=auto` で window context を集約表示
    - `full` は 1行/window を維持したまま各 window 行に `cwd`/`branch` を常時表示
    - context 不一致/欠損混在は `[field=<mixed>] ...` を表示
    - ガイダンス `(use --context=full to expand per-pane values)` は session block で 1 回のみ（session mixed 優先）
    - fzf → `tmux select-window -t "session:window_name"` (@N 不要)
  - `list-sessions` = session 単位の集計。1行/session。
    - `session_name  N windows  [~] status`
    - default `--context=auto`。context は header 集約で最小表示
    - `full` は 1行/session を維持したまま各 session 行に `cwd`/`branch` を常時表示
    - context 不一致/欠損混在は `[field=<mixed>] ...` を表示
    - ガイダンス `(use --context=full to expand per-pane values)` は session block で 1 回のみ（session mixed 優先）
    - fzf → `tmux switch-client -t session_name`
  - summary は `--summary` 指定時のみ表示（default off）
    - agent 明示の構造化 summary（AppServer/hooks/JSONL）由来のみ採用
    - capture/title 推測値は summary に混ぜない（誤誘導防止）
    - pane 行の直下に 1 行で表示（summary欠損 pane は行を出さない）。全 pane 欠損時のみ全出力末尾に 1 回だけ `(no agent summaries available)` を表示
  - single-window session の `list-panes` は window header を出さない（session 直下に pane を表示）
  - conversation title (T-135) は後続タスク。追加後に `list-panes` の title フィールドが自動充足
- Deliverables:
  - T-130 ✅: `build_pane_list` に不足フィールド追加（`window_id`, `session_id`, `current_path`）
  - T-131 ✅: `agtmux list-windows` コマンド初版（T-134 でリデザイン）
  - T-132 ✅: fzf レシピ + README 初版（T-134 完了後に更新）
  - T-133: `list-panes` 表示リデザイン — sidebar-style human output、heur `~` marker、`--json` + context controls
  - T-134: `list-windows` リデザイン + `list-sessions` 新規 — tcmux スタイル、@N 非表示
  - T-135a: Codex conversation title 抽出 — `thread/list` の `name` フィールドを `conversation_titles` map 経由で `build_pane_list` に追加（最小変更: poll_loop + server の 2 ファイル）
  - T-135b: Claude JSONL conversation title 抽出 — `sessions-index.json` から title を同一 map に挿入
  - T-139: `--context=auto|off|full` + header context compaction（pane差分のみ表示）
  - T-140: `list-windows` / `list-sessions` の context 集約 + `mixed` marker + fzf parse contract
  - T-141: `--summary` opt-in 表示（deterministic source only）+ summary placement contract
  - T-142: CLI UX output contracts（golden fixtures: 全一致 / branch混在 / cwd混在 / window-header-fallback / summary全欠損）
  - (Post) TUI (ratatui) — インタラクティブ版（CLI で価値確認後に検討）
  - (Post) GUI — サイドバー + tty パネル（TUI で価値確認後に検討）
- Exit criteria:
  - `agtmux list-panes` がサイドバーと同等の情報を人間向けに表示（heur `~` 付き）
  - `agtmux list-windows` / `list-sessions` が tcmux スタイルで表示
  - default 出力で情報過多にならず、`--context` / `--summary` で必要時のみ情報密度を上げられる
  - `--path`/`-p` に依存せず `--context=full` の単一メンタルモデルで理解できる
  - `mixed` 表示から `--context=full` への導線が明示される（`use --context=full to expand per-pane values`）
  - fzf パイプで tmux ウィンドウ/セッション切り替えが動作
  - 自分で日常的に使えるレベル

## Phase 7: Distribution Infrastructure (新規)

- Goal: 初日から `brew install g960059/tap/agtmux` が通る状態を作る。詳細 → `docs/55_distribution.md`
- Design: cargo-dist + GitHub Actions + Homebrew tap + musl static binary
- Deliverables:
  - T-D01: LICENSE + Cargo.toml メタデータ整備（description, repository, license, keywords, categories）
  - T-D02: cargo-dist 設定（`[workspace.metadata.dist]` + `cargo dist init`）
  - T-D03: GitHub Actions release workflow（`.github/workflows/release.yml`）
  - T-D04: Homebrew formula テンプレート（`Formula/agtmux.rb`）
  - T-D05: README Install セクション更新（brew / curl / cargo の3チャネル）
- Exit criteria:
  - `git tag v0.1.0 && git push --tags` で GitHub Release + Homebrew formula 更新が自動実行される
  - `brew install g960059/tap/agtmux && agtmux --version` が通ること
  - README の Install セクションに3チャネル（brew / curl / cargo）が記載されていること
  - `just verify` PASS

## Risks / Mitigations
- Risk: docs 先行で実装が遅れる
  - Mitigation: `[MVP]` のみで Phase 1-2 を完了させる
- Risk: hardening 未実装で運用課題が残る
  - Mitigation: 課題が再現した時点で Post-MVP タスクを昇格する

## Rollout
- v4 は production に進んでいないため、段階移行戦略は不要。
- CLI/TUI を自分で日常的に使い始める時点が実質的な "alpha"。
- GUI バンドルは CLI/TUI で価値確認後に検討する。
