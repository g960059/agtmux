# AGTMUXDesktop (macOS app)

`AGTMUXDesktop` は AGTMUX の daemon (`agtmuxd`) と app CLI (`agtmux-app`) を内包して起動できる SwiftUI デスクトップアプリです。  
アプリ起動時に daemon の疎通確認を行い、必要なら自動起動します。

## できること

- Targets / Sessions / Windows / Panes の一覧表示
- pane 状態ごとのボード表示（running / waiting / completed など）
- pane への送信（send）
- pane 出力の取得（view-output）
- pane 停止（kill: key INT / signal TERM）
- アプリから daemon 再起動（所有プロセス）

## 前提

- macOS 14+
- Go 1.25+
- Swift 6 系 + macOS SDK（通常は Xcode 本体のインストールを推奨）

## 使い方（開発実行）

リポジトリルートで:

```bash
./macapp/scripts/run-dev.sh
```

このスクリプトは次を実行します。

1. `agtmux`, `agtmuxd`, `agtmux-app` を `macapp/.runtime/bin` にビルド
2. `AGTMUX_DAEMON_BIN`, `AGTMUX_APP_BIN` を設定
3. `swift run AGTMUXDesktop` でアプリ起動

## .app を作る（同梱バイナリ付き）

```bash
./macapp/scripts/package-app.sh
```

生成先:

- `macapp/dist/AGTMUXDesktop.app`

同梱される実行ファイル:

- `Contents/MacOS/AGTMUXDesktop`
- `Contents/Resources/bin/agtmuxd`
- `Contents/Resources/bin/agtmux-app`

## インストール

ユーザー領域（推奨）にインストール:

```bash
./macapp/scripts/install-app.sh
```

デフォルトインストール先:

- `~/Applications/AGTMUXDesktop.app`

起動:

```bash
open ~/Applications/AGTMUXDesktop.app
```

## データ保存先

アプリ実行時に以下を使用します。

- Socket: `~/Library/Application Support/AGTMUXDesktop/agtmuxd.sock`
- DB: `~/Library/Application Support/AGTMUXDesktop/state.db`
- Log: `~/Library/Application Support/AGTMUXDesktop/agtmuxd.log`

## バイナリ解決順

`agtmuxd` / `agtmux-app` は次の順で探索します。

1. `AGTMUX_DAEMON_BIN`, `AGTMUX_APP_BIN`
2. App bundle (`Contents/Resources/bin/...`)
3. 現在ディレクトリ (`./bin/...` など)
4. `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`
5. `PATH` + `which`

## トラブルシュート

- `Binary not found`:
  - `AGTMUX_DAEMON_BIN` / `AGTMUX_APP_BIN` を明示設定して起動してください。
- Swift build の toolchain/SDK エラー:
  - Command Line Tools のみでは不整合が出る場合があります。Xcode 本体を入れて `xcode-select` を合わせてください。
