# AGTMUX

tmux 上で動くエージェント（Codex / Claude / Gemini など）を、1つの daemon API と CLI で観測・操作するための Go 実装です。

このリポジトリは実装 PoC 段階です。仕様と計画は `docs/` にあります。

- 仕様: `docs/agtmux-spec.md`
- 実装計画: `docs/implementation-plan.md`
- タスク: `docs/tasks.md`
- テストカタログ: `docs/test-catalog.md`
- mac app 状態/UI設計: `docs/macapp-ui-state-model.md`
- event-driven 連携: `docs/event-driven-integration.md`
- Phase 1-6 実装記録/意思決定: `docs/implementation-records/phase1-6-implementation-decision-log-2026-02-15.md`

## 前提

- Go 1.25+
- tmux 3.3+（推奨）
- macOS / Linux

## インストール

ローカルに 3 つのバイナリを作る例です。

```bash
mkdir -p ./bin
go build -o ./bin/agtmux ./cmd/agtmux
go build -o ./bin/agtmuxd ./cmd/agtmuxd
go build -o ./bin/agtmux-app ./cmd/agtmux-app
```

`$PATH` に置く場合:

```bash
go install ./cmd/agtmux
go install ./cmd/agtmuxd
go install ./cmd/agtmux-app
```

## クイックスタート

1. daemon 起動:

```bash
./bin/agtmuxd
```

`--socket` と `--db` で保存先を指定できます。

```bash
./bin/agtmuxd --socket /tmp/agtmuxd.sock --db /tmp/agtmux.db
```

2. ターゲット確認:

```bash
./bin/agtmux target list
```

3. event-driven 連携を初期設定（Claude hooks / Codex notify / wrapper）:

```bash
./bin/agtmux integration install
```

4. 一覧表示:

```bash
./bin/agtmux list panes --json
./bin/agtmux list windows --json
./bin/agtmux list sessions --json
```

5. watch:

```bash
./bin/agtmux watch --scope panes --json --once
```

6. アクション:

```bash
./bin/agtmux send --request-ref req1 --target local --pane %1 --text "hello" --enter
./bin/agtmux view-output --request-ref req2 --target local --pane %1 --lines 50
./bin/agtmux kill --request-ref req3 --target local --pane %1 --mode key --signal INT
```

7. イベント送信（wrapper/hook の動作確認）:

```bash
./bin/agtmux event emit --target local --pane %1 --agent codex --source wrapper --type wrapper-start
```

8. アクション監査イベント:

```bash
./bin/agtmux events --action-id <action_id>
```

## デフォルトパス

- socket:
  - `$XDG_RUNTIME_DIR/agtmux/agtmuxd.sock`
  - `XDG_RUNTIME_DIR` が無い場合は `~/.local/state/agtmux/agtmuxd.sock`
- DB:
  - `~/.local/state/agtmux/state.db`

## Database Rollback

`states` テーブルの provenance 追加（`state_source`, `last_event_type`, `last_event_at`）は additive migration です。  
SQLite 環境差分を避けるため、ロールバック時は DB 再生成方式を前提とします。

```bash
rm -f ~/.local/state/agtmux/state.db
# その後 daemon を再起動（tmux から再同期）
```

## `agtmux` コマンド

基本形:

```bash
agtmux [--socket <path>] <command> ...
```

サポート済み command:

- `target <list|add|connect|remove>`
- `adapter <list|enable|disable>`
- `event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>] [--json]`
- `integration install [--dry-run] [--json] [--force-codex-notify] [--skip-claude] [--skip-codex] [--skip-wrappers]`
- `integration doctor [--home <dir>] [--bin-dir <dir>] [--json]`
- `list <panes|windows|sessions> [--target <name>] [--json]`
- `watch [--scope <panes|windows|sessions>] [--target <name>] [--cursor <stream:seq>] [--json] [--once]`
- `send --request-ref <id> --target <name> --pane <id> (--text <text>|--key <key>|--stdin) [--enter] [--paste] [guard flags] [--json]`
- `view-output --request-ref <id> --target <name> --pane <id> [--lines <n>] [guard flags] [--json]`
- `kill --request-ref <id> --target <name> --pane <id> [--mode key|signal] [--signal INT|TERM|KILL] [guard flags] [--json]`
- `events --action-id <id> [--json]`
- `app ...`（`agtmux-app` を呼び出し）

`guard flags`:

- `--if-runtime <runtime_id>`
- `--if-state <state>`
- `--if-updated-within <duration>`
- `--force-stale`

### `send --stdin` の制約

- TTY からの直接入力は不可（pipe 必須）
- 空入力は不可
- 最大 1 MiB

例:

```bash
printf 'line1\nline2\n' | agtmux send --request-ref req4 --target local --pane %1 --stdin
```

## `agtmux app`（常駐 app 呼び出し導線）

`agtmux app ...` は `agtmux-app ...` へのパススルーです。

- `--socket` は自動で先頭に引き継ぎます
- 子プロセスの終了コードをそのまま返します
- 実行バイナリは `AGTMUX_APP_BIN` で上書きできます（デフォルト `agtmux-app`）

例:

```bash
agtmux --socket /tmp/agtmuxd.sock app run --once --json
AGTMUX_APP_BIN=./bin/agtmux-app agtmux app view global --json
```

## Event-Driven 連携セットアップ

`integration install` は以下をまとめて設定します。

- `~/.claude/settings.json` に hook command を追加（`Notification`/`Stop`/`SubagentStop`）
- `~/.codex/config.toml` の `notify` を設定（既存 notify がある場合は上書きせず warning）
- `~/.local/share/agtmux/bin` に wrapper / hook helper を配置
  - `agtmux-codex`
  - `agtmux-claude`
  - `agtmux-hook-emit`
  - `agtmux-codex-notify`

ドライラン:

```bash
agtmux integration install --dry-run --json
```

導入状態の検査:

```bash
agtmux integration doctor --json
```

Codex notify を強制置換:

```bash
agtmux integration install --force-codex-notify
```

ラッパー起動例:

```bash
~/.local/share/agtmux/bin/agtmux-codex
~/.local/share/agtmux/bin/agtmux-claude
```

## `agtmux-app` コマンド

基本形:

```bash
agtmux-app [--socket <path>] [--request-timeout <duration>] [run ...]
```

`run` を省略すると resident loop (`run`) として動きます。

サポート済みサブコマンド:

- `run [--scope panes|windows|sessions] [--target <name>] [--poll-interval <dur>] [--cursor <cursor>] [--once] [--json]`
- `view <snapshot|global|sessions|windows|panes|targets> ...`
- `target <list|add|connect|remove> ...`
- `action <attach|send|view-output|kill|events> ...`
- `adapter <list|enable|disable> ...`

例:

```bash
agtmux-app run --scope panes --json --once
agtmux-app view global --follow
agtmux-app action send --request-ref req10 --target local --pane %1 --text "status"
```

## macOS デスクトップ app (`AGTMUXDesktop`)

SwiftUI の常駐向け UI を `macapp/` に用意しています。  
アプリ起動時に daemon を疎通確認し、必要なら自動起動します。

詳細: `macapp/README.md`

最短実行:

```bash
./macapp/scripts/run-dev.sh
```

.app 作成:

```bash
./macapp/scripts/package-app.sh
```

インストール（`~/Applications`）:

```bash
./macapp/scripts/install-app.sh
```

## 開発

テスト:

```bash
go test ./...
go test -race ./...
```

主要ディレクトリ:

- `cmd/agtmux`: CLI エントリポイント
- `cmd/agtmuxd`: daemon エントリポイント
- `cmd/agtmux-app`: resident app CLI
- `internal/daemon`: HTTP/UDS API サーバー
- `internal/cli`: `agtmux` CLI 実装
- `internal/appclient`: `agtmux-app` から daemon を叩くクライアント
- `internal/db`: SQLite ストア

## 終了コード（目安）

- `0`: 成功
- `1`: 実行時エラー（通信・API・子プロセス失敗など）
- `2`: 引数/使用法エラー
