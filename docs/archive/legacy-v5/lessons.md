# Lessons Learned (Self-Improvement Log)

> ユーザーから修正を受けたら、ここにパターンを記録する。
> セッション開始時に関連 lessons があれば確認する。
> 形式: `## YYYY-MM-DD — <タイトル>` + 原因・教訓・防止策。

---

## 2026-02-27 — Subagent delegation の遵守率が低い

**状況**: T-121〜T-126 の実装全般で、Orchestrator がコード実装・テスト実行・レビューを直接行い、
subagent への委任をほぼしなかった（遵守率 ~35%）。

**根本原因**:
- 委任ルールが `docs/00_router.md` にのみ記載 → auto-load されない → 実質的に守られない
- "少しだけ直接修正する" という小さな逸脱が積み重なる

**教訓**: 守られるべきルールは CLAUDE.md（常時ロード）に書く。router.md のみは不十分。

**防止策**: CLAUDE.md Hard Gate #1 に明記済み。

---

## 2026-02-27 — Plan 承認後に docs 更新をスキップして実装に直行

**状況**: Plan mode で `.claude/plans/` にプランを作成 → ユーザー承認 → そのまま実装開始。
`docs/60_tasks.md` / `docs/70_progress.md` が更新されないまま実装が進む。

**根本原因**:
- Plan mode のアウトプット（`.claude/plans/`）が "公式" に見える
- docs 更新は意識的な摩擦ステップであり、実装モメンタムに負ける

**教訓**: `.claude/plans/` は scratch。承認後の最初のアクションは docs 更新と plan ファイル削除。

**防止策**: CLAUDE.md Hard Gate #2 に明記済み。

---

## 2026-02-27 — Multi-phase タスクの Phase 2/3 が docs に反映されない

**状況**: T-126 が 3 phase 構成だったが、コンテキスト圧縮後に Phase 2/3 の詳細が
`docs/70_progress.md` に残っておらず、次セッションでの再確認が困難だった。

**根本原因**: "タスク完了時にまとめて書く" という defer 習慣。
コンテキスト圧縮がフェーズ間で発生すると情報が失われる。

**教訓**: 各フェーズ完了直後に書く。defer は情報損失と等価。

**防止策**: CLAUDE.md Hard Gate #3 に明記済み。

---

## 2026-02-27 — Review Pack なし・reviewer なし・GO 判定なしでコミット

**状況**: T-121〜T-126 全タスクで Review Pack が作成されず、Codex reviewer も呼ばれず、
そのまま commit / push が行われた。

**根本原因**: Review ルールが router.md の深い場所にのみ記載。
`just verify` PASS = 完了 という誤認識。

**教訓**: verify は最低限の gate。review は別の独立した gate。

**防止策**: CLAUDE.md Hard Gate #4 に明記済み。

---

## 2026-02-27 — 実装レベルの fallback 多用による根本原因の隠蔽

**状況**: JSONL path encoding、pane detection、CWD 解決などで
「失敗したら別の方法で推測」するパターンが複数重なり、
どこで何が失敗しているかが見えなくなった（T-122, T-126 等）。

**根本原因**: "なんとか動く" を目標にした防衛的実装。
監視ツールでは silent wrong answer が silent no answer より危険。

**教訓**: 実装レベルの fallback は silent failure を作る。エラーを surface せよ。
アーキテクチャ設計上の tier 降格のみ許可。

**防止策**: CLAUDE.md Code Quality Policy に明記済み。

---

## 2026-03-01 — テスト用スクリプトが PATH 上の stale binary を使い誤デバッグ

**状況**: `daemon.sh` が `AGTMUX_BIN="agtmux"` (デフォルト値) を受け取ると、
`[ "${AGTMUX_BIN:-}" != "" ]` が真になり、`command -v agtmux` でグローバルインストール
(`/usr/local/bin/agtmux` 等) を発見 → デーモンが stale binary で起動。
JSONL スキャナーなどの新機能が動かず、テストが謎の失敗を繰り返した。

**根本原因**: `common.sh` が `AGTMUX_BIN="${AGTMUX_BIN:-agtmux}"` (コマンド名をデフォルト設定)。
`daemon.sh` の空文字チェックは意図通り動いているが、"agtmux" がグローバルバイナリにヒットする。

**教訓**:
1. e2e ハーネスでのバイナリ解決は「絶対パスのみを外部指定として認める」が安全。
2. デーモン起動後にクライアントヘルパー (`jq_get` 等) も同じバイナリを使うよう `AGTMUX_BIN` を更新する。
3. デーモンログが「ポーリングメッセージなし」なら使用バイナリのビルド日時を最初に確認する。

**防止策**: `daemon.sh` を `[[ "$_daemon_bin" == /* ]]` で絶対パス判定に修正。
ローカルビルド後 `AGTMUX_BIN="$_built_bin"` を上書き。

---

## 2026-03-02 — ローカル lint と CI clippy のフラグ不一致が「ローカル PASS、CI FAIL」を生む

**状況**: v0.1.5 の CI が `clippy::unnecessary_map_or` で失敗。ローカル `just lint` は PASS していた。

**根本原因**: `justfile` の `lint` ターゲットが特定の lint フラグ (`-D clippy::dbg_macro` 等) を指定していたが
`-D warnings` を含んでいなかった。CI は `cargo clippy --workspace -- -D warnings` で実行するため、
CI の Rust バージョンで追加された新しい lint が警告扱いになると CI だけで失敗する。

**教訓**:
1. ローカル `lint` は CI と完全に同じフラグを使うこと。差異があると「ローカル PASS → CI FAIL → 小さな fix commit」サイクルが発生する。
2. `-D warnings` はすべての future lint もエラーにするため、CI で使う場合はローカルにも必ず適用する。
3. pre-commit hook に `cargo clippy -- -D warnings` を入れることで commit 時点でキャッチできる。

**防止策**:
- `justfile` の `lint` に `-D warnings` を追加 (CI と一致)
- `scripts/pre-commit.sh` に clippy を追加 (`cargo clippy --workspace -- -D warnings`)
- `just install-hooks` でクローン後にワンコマンドで設定できるようにした
- `docs/55_distribution.md` にローカル開発リリースフローを明記し、エージェントが遵守できるようにした

---

## 2026-03-02 — CWD-based JSONL detection が「共有 CWD」と「日付外ディレクトリ」で誤動作する

**状況**: v0.1.9 で `is_file_write_open()` を追加して Codex JSONL の write-open 判定を追加したが、
- **False positive**: 同じ CWD を持つ別 pane の background Codex プロセスが write-open を返してしまい、
  無関係 pane が `Running` 誤表示された（`vm agtmux v4` panes）
- **False negative**: test-session の Codex は JSONL が 7 日前のディレクトリにあり、
  today/yesterday スキャン範囲に入らず `Idle` 誤表示された（`/2026/02/23/` JSONL）

**根本原因**: CWD ベースのマッチングは「そのパネルのプロセスが JSONL を持っているか」を確認できない。
任意のプロセスが同 CWD にいれば誤マッチする。またスキャン範囲（today/yesterday）は外部から
制約されていて、長期実行セッションを取りこぼす。

**教訓**:
1. "Which process owns this JSONL" は CWD では解けない — PID lineage で解く必要がある。
2. `is_file_write_open(path)` はファイルを開いているプロセスを特定しない → CWD 共有で誤答。
3. 日付ディレクトリ制限を回避するには、プロセスから JSONL を逆引きする必要がある。

**修正 (v0.1.10)**:
- **Process-first detection** (Pass 1): `pane_pid` → `process_map` で Codex 子プロセス探索
  → `lsof -p <codex_pid> -Fan` で open JSONL を特定 → CWD マッチ・日付制限なしで確実に紐付け
- CWD ベーススキャン (Pass 2) は fallback に格下げし、`is_file_write_open` を削除
- `PaneCwdInfo` に `pane_pid: Option<u32>` フィールド追加、`ProcessMap` を scanner に渡す

---

## 2026-03-02 — Codex burst-write で Pass 1 が空振り・Pass 3 が fresh ファイルをスキップ

**状況**: v0.1.10 で test-session codex が依然 Idle 誤表示。
- Codex は open→write→close のバースト書き込みを行うため、lsof では JSONL が見えないことが多い
  → Pass 1 (process-first) が空振り
- Pass 3 (historical enrichment) は `age_secs <= JSONL_IDLE_THRESHOLD_SECS` をスキップするため、
  古い日付ディレクトリ（`2026/02/23`）内の fresh ファイル（age=4s）も除外してしまう
- `MAX_CWD_QUERIES_PER_TICK = 1` により、複数 CWD がある場合に test-session CWD が飢餓する

**根本原因**:
1. Pass 3 の「fresh ファイルスキップ」は Pass 2 がカバーしていることを前提にしているが、
   Pass 2 は today/yesterday しかスキャンしないため、古い日付ディレクトリを見逃す。
2. App Server の CWD クエリが 1 件/tick に制限され、2 つ以上の CWD があると後続が飢餓する。

**修正 (v0.1.11)**:
- Pass 3 の `age_secs <= JSONL_IDLE_THRESHOLD_SECS` スキップを削除。
  代わりに mtime ブランチを追加: `age <= 25s → thread.active`、それ以外 `→ thread.idle`。
- `MAX_CWD_QUERIES_PER_TICK` を 1 → 3 に増加、外側タイムアウトを 8s → 16s に拡張。
