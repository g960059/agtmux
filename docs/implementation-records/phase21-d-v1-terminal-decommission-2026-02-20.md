# Phase 21-D: v1 Terminal API Decommission (2026-02-20)

## Goal
- Phase 21-D の要件どおり、`/v1/terminal/*` を本番経路から外し、`tty-v2` を既定経路に固定する。

## Decisions
1. Daemon の `v1 terminal` route はデフォルト無効。
2. 旧 route が必要な回帰テストのみ、明示フラグで有効化。
3. macapp の `AppViewModel` は `allowTerminalV1Fallback` の既定値を `false` に変更。
4. `AGTMUXDesktopApp` は引き続き `allowTerminalV1Fallback: false` を明示指定。

## Implementation
- `internal/config/config.go`
  - `Config.EnableLegacyTerminalV1 bool` を追加。
  - `DefaultConfig()` 既定値を `false` に設定。
- `internal/daemon/server.go`
  - `cfg.EnableLegacyTerminalV1` が `true` の場合のみ `/v1/terminal/*` を route 登録。
- `internal/daemon/server_test.go`
  - `newAPITestServer` では回帰互換のため `EnableLegacyTerminalV1 = true` を指定。
  - `TestTerminalV1EndpointsDisabledByDefault` を追加（既定では 404）。
  - `TestTerminalV1EndpointsEnabledWithConfig` を追加（有効時は route 登録され 405 応答）。
- `macapp/Sources/AppViewModel.swift`
  - `allowTerminalV1Fallback` のデフォルト値を `false` に変更。
- `macapp/Tests/AppViewModelSettingsTests.swift`
  - `v1` 判定テストを「fallback 無効時は拒否」「fallback 有効時は受理」に分離。
- `macapp/Tests/AppViewModelTerminalProxyTests.swift`
  - v1 経路テスト fixture は `allowTerminalV1Fallback: true` を明示。

## Verification
- `go test ./...` PASS
- `cd macapp && swift test` PASS
- 実機確認（インストール済み app + daemon）
  - `terminal capabilities` は `terminal_frame_protocol: "tty-v2"`
  - `POST /v1/terminal/read` は `404 page not found`

## Notes
- 旧 `v1 terminal` 実装コード自体はまだ残っており、次段で削除可能。
- ただし実運用経路ではデフォルトで到達不能化済み。
