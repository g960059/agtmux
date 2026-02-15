# Event-Driven Integration (Hooks / Notify / Wrapper)

## 目的

`agtmux` の状態推定を poller 主体から event-first 主体へ移し、agent の実状態をより安定して反映する。

## 取り込み経路

- daemon: `POST /v1/events`
- CLI: `agtmux event emit ...`
- source: `hook | notify | wrapper | poller`

`runtime_id` が無いイベントは `target + pane` で `pending_bind` として受理される。  
active runtime がヒント付きで一意に決まる場合のみ即時 `bound` で適用される。

## install 時に設定されるもの

`agtmux integration install` は以下を管理する。

- `~/.claude/settings.json`
  - `Notification` / `Stop` / `SubagentStop` に command hook を追加
- `~/.codex/config.toml`
  - `notify = ["sh", "-lc", "<...>/agtmux-codex-notify \"$1\""]`
- `~/.local/share/agtmux/bin`
  - `agtmux-codex`
  - `agtmux-claude`
  - `agtmux-hook-emit`
  - `agtmux-codex-notify`

## 安全性

- idempotent: 再実行しても壊れない
- backup: 既存設定を変更する場合は `.bak.<timestamp>` を作る
- atomic write: `tmp + rename`
- dry-run: `--dry-run` で非破壊プラン確認

## 注意点

- 既存 `~/.codex/config.toml` に `notify` が既にある場合は、デフォルトで上書きしない（warning を返す）。
- 強制置換する場合のみ `--force-codex-notify` を使う。
