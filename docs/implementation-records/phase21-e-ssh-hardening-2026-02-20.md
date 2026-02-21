# Phase 21-E: SSH Target Hardening (2026-02-20)

## Goal
- `tty-v2` の stream loop で、SSH target の遅延・ジッタ時に daemon が過剰再試行/過剰エラー送信しないようにする。
- multi-target / multi-pane 時の DB 解決コストを減らし、foreground pane の反応を安定化する。

## Changes

### 1) Capture scheduler hardening
- `internal/daemon/tty_v2.go`
  - `ttyV2AttachedPane` に以下を追加:
    - `nextCaptureAt`
    - `captureFailures`
    - `lastErrorAt`
  - capture 失敗時に指数バックオフ:
    - local foreground/background base
    - ssh foreground/background base
    - local / ssh で max backoff を分離
  - error frame の送信を throttle:
    - foreground: 1.2s
    - background: 3s
  - capture 成功時に failure/backoff state を即クリア

### 2) SSH interval tuning
- `internal/daemon/tty_v2.go`
  - background capture interval を target kind で分離:
    - local: `250ms`
    - ssh: `450ms`

### 3) Bulk resolve for attached panes
- `internal/daemon/tty_v2.go`
  - `resolveAttachedRefs` を追加。
  - `pushOutputs` で毎paneごとの `GetTargetByName + ListPanes` を繰り返すのを廃止し、1回の bulk 読み出しで pane ref を解決。
  - bulk resolve 失敗時は既存の per-pane resolve にフォールバック。

## Tests
- `internal/daemon/tty_v2_scheduler_test.go`
  - `TestTTYV2ShouldCaptureOutputSSHUsesLongerBackgroundInterval`
  - `TestTTYV2ShouldCaptureOutputRespectsBackoff`
  - `TestTTYV2RecordCaptureFailureBackoffAndThrottle`
  - `TestTTYV2CaptureSuccessClearsFailureBackoff`
  - 既存 `TestTTYV2ShouldCaptureOutputForegroundVsBackground` を新シグネチャに更新。

## Verification
- `go test ./internal/daemon` PASS
- `go test ./...` PASS
- `cd macapp && swift test` PASS
- app rebuild/restart 済み
  - capabilities: `terminal_frame_protocol = "tty-v2"`

## Notes
- この段では stream protocol 仕様は変更せず、scheduler と実行負荷の安定化を先に適用。
- 次段で必要なら、capture 実行の並列度制御・target別 QoS を追加できる。
