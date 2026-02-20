# Phase 21-A: `/v2/tty/session` Wire Schema (2026-02-20)

Status: Draft v2.0 (implementation target)
Related:
- `docs/implementation-records/phase21-tty-v2-terminal-engine-detailed-design-2026-02-20.md`
- review input: `/tmp/phase21-tty-v2-design-review.md`
- review input: `/tmp/phase21-wire-schema-and-design-review.md`

## 1. Scope

本仕様は `AGTMUXDesktop <-> agtmuxd` 間の TTY v2 双方向ストリームを定義する。

- transport: UDS persistent stream
- protocol: length-prefixed JSON frames
- coverage: hello/attach/write/resize/focus/detach/ping/resync + output/state/ack/error

Non-goal:

- SSH hop 内部プロトコル
- tmux control mode 生イベント仕様

---

## 2. Transport

## 2.1 Endpoint

- path: `/v2/tty/session`
- connection: 1 client connection = 1 long-lived duplex stream

## 2.2 Framing

- frame unit: `uint32_be length` + `json_utf8_payload`
- max frame size: `1_048_576` bytes (1 MiB)
- heartbeat: client `ping` every 10s (idle時), server `pong`

If invalid length or invalid JSON:

- server closes stream with `error(code="e_protocol_invalid_frame")` when possible

---

## 3. Common Envelope

すべてのフレームは共通 envelope を持つ。

```json
{
  "schema_version": "tty.v2.0",
  "type": "hello",
  "frame_seq": 1,
  "sent_at": "2026-02-20T12:34:56.123Z",
  "request_id": "req-optional",
  "payload": {}
}
```

Fields:

- `schema_version` string, required, fixed `tty.v2.0`
- `type` string, required
- `frame_seq` uint64, required, sender-local monotonic
- `sent_at` RFC3339Nano UTC string, required
- `request_id` string, optional
- `payload` object, required

Rules:

1. `frame_seq` は接続単位で単調増加。
2. `request_id` は request/ack/error 相関用。event系は省略可。
3. unknown field は受信側で無視（forward-compatible）。
4. `frame_seq` は観測/デバッグ用途。ack/再送制御には使わない。

---

## 4. Shared Payload Objects

## 4.1 `pane_ref`

```json
{
  "target": "local",
  "session_name": "exp-go-codex-implementation-poc",
  "window_id": "@3",
  "pane_id": "%10"
}
```

- all fields required, non-empty
- canonical key: `target|session_name|window_id|pane_id`

## 4.2 `tty_state`

```json
{
  "activity_state": "running",
  "attention_state": "none",
  "session_last_active_at": "2026-02-20T12:34:20.000Z"
}
```

Enums:

- `activity_state`: `running|idle|waiting_input|waiting_approval|error|unknown`
- `attention_state`: `none|action_required_input|action_required_approval|action_required_error|informational_completed`

## 4.3 `pane_alias`

`pane_ref` の冗長性を削減するための短縮ID。接続内スコープで有効。

例:

- `p1`
- `p2`

Rules:

1. `attached` で daemon が払い出す。
2. client は `pane_alias` capability が双方で有効な場合にのみ使用する。
3. alias 未対応時は `pane_ref` のみで通信する。

---

## 5. Client -> Daemon Frames

## 5.1 `hello`

Payload:

```json
{
  "client_id": "agtmux-desktop",
  "protocol_versions": ["tty.v2.0"],
  "capabilities": [
    "raw_output",
    "resync",
    "focus",
    "resize_conflict_ack",
    "pane_alias",
    "binary_frames"
  ]
}
```

Validation:

- `protocol_versions` must include `tty.v2.0`
- empty capabilities allowed

## 5.2 `attach`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "attach_mode": "live",
  "want_initial_snapshot": true
}
```

- `attach_mode`: `live` only (reserved for future)
- `want_initial_snapshot`: bool, default `true`

## 5.3 `write`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "input_seq": 1024,
  "bytes_base64": "Gxtr"
}
```

Rules:

1. `input_seq` monotonic per `pane_ref` per client.
2. empty bytes invalid.

## 5.4 `resize`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "resize_seq": 55,
  "cols": 168,
  "rows": 42
}
```

Validation:

- `20 <= cols <= 500`
- `5 <= rows <= 300`

## 5.5 `focus`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" }
}
```

Effects:

- marks sender as authoritative focus client for `pane_ref`

## 5.6 `detach`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" }
}
```

## 5.7 `resync`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "reason": "sequence_gap"
}
```

`reason` enum:

- `sequence_gap`
- `output_decode_error`
- `client_reconnect`
- `manual`

## 5.8 `ping`

Payload:

```json
{ "ts": "2026-02-20T12:35:00.000Z" }
```

---

## 6. Daemon -> Client Frames

## 6.1 `hello_ack`

Payload:

```json
{
  "server_id": "agtmuxd",
  "protocol_version": "tty.v2.0",
  "features": [
    "raw_output",
    "resync",
    "peer_cred_auth",
    "resize_conflict_ack",
    "pane_alias",
    "binary_frames"
  ]
}
```

## 6.2 `attached`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "pane_alias": "p1",
  "output_seq": 200,
  "initial_snapshot_ansi_base64": "Li4u",
  "state": {
    "activity_state": "idle",
    "attention_state": "none",
    "session_last_active_at": "2026-02-20T12:34:20.000Z"
  }
}
```

`initial_snapshot_ansi_base64` may be empty when unavailable.
`pane_alias` is optional (required only when `pane_alias` capability is negotiated).

## 6.3 `output`

Payload:

```json
{
  "pane_alias": "p1",
  "output_seq": 201,
  "bytes_base64": "G1szMW0uLi4=",
  "coalesced": true,
  "coalesced_from_seq": 198,
  "dropped_chunks": 2
}
```

Rules:

1. `output_seq` monotonic per pane.
2. when backlog occurs, server may send coalesced output and increment `dropped_chunks`.
3. `coalesced=true` の場合、`output_seq` のジャンプは正常動作。client は resync しない。
4. `coalesced=true` の場合、`coalesced_from_seq` を必須とする。
5. `coalesced=false` で gap (`seq != last+1`) を検知した場合のみ、client は `resync(reason=sequence_gap)` を送る。
6. `pane_alias` と `pane_ref` はどちらか一方を必須とし、両方送ってもよい（migration期間）。

## 6.4 `state`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "state": {
    "activity_state": "running",
    "attention_state": "none",
    "session_last_active_at": "2026-02-20T12:35:02.111Z"
  }
}
```

## 6.5 `ack`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "ack_kind": "write",
  "input_seq": 1024,
  "result_code": "ok"
}
```

`ack_kind` enum: `write|resize|focus|detach|resync`

`result_code` enum:

- `ok`
- `skipped_conflict` (resize conflict)
- `stale_runtime`
- `not_attached`

## 6.6 `error`

Payload:

```json
{
  "code": "e_ref_not_found",
  "message": "pane not found",
  "recoverable": true,
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" }
}
```

Core error codes:

- `e_protocol_invalid_frame`
- `e_protocol_unsupported_version`
- `e_auth_forbidden`
- `e_ref_not_found`
- `e_runtime_stale`
- `e_tmux_bridge_down`
- `e_tmux_write_failed`
- `e_tmux_resize_failed`
- `e_stream_backpressure`
- `e_internal`

## 6.7 `detached`

Payload:

```json
{
  "pane_ref": { "target": "local", "session_name": "s", "window_id": "@1", "pane_id": "%2" },
  "reason": "client_detach"
}
```

`reason` enum: `client_detach|pane_killed|target_down|bridge_restart|server_shutdown`

## 6.8 `pong`

Payload:

```json
{ "ts": "2026-02-20T12:35:00.000Z" }
```

---

## 7. Ordering and Guarantees

1. Per-pane `output_seq` is strictly increasing.
2. `ack(write,input_seq=n)` is emitted after server accepted write for pane stream.
3. `state` events are eventually consistent; not guaranteed per output frame.
4. `coalesced=true` frames may skip intermediate sequences by design.
5. Exactly-once is not guaranteed across reconnect; client must support resync.

---

## 8. Focus and Resize Conflict Policy

1. Last client that sent `focus` for pane becomes authoritative.
2. Non-authoritative `resize` receives `ack(result_code="skipped_conflict")`.
3. Non-authoritative `write` is allowed (interactive collaboration).
4. When authoritative client disconnects, authority clears until next `focus`.
5. focus authority is implicitly released by `detach` or by sending `focus` for another pane.

---

## 9. Auth and Trust Boundary

1. server obtains peer credentials via `getpeereid` (macOS UDS).
2. only allowed uid (app user) may connect.
3. optional hardening: verify code-sign identity for desktop app process path.
4. protocol-level `session_token` is not mandatory in v2.0.

---

## 10. Flow Examples

## 10.1 Initial Attach

1. client `hello`
2. server `hello_ack`
3. client `attach`
4. server `attached(initial_snapshot_ansi_base64, output_seq=n)`
5. server `output(output_seq=n+1...)`

## 10.2 Input

1. client `write(input_seq=77, bytes=...)`
2. server `ack(write,input_seq=77,result_code=ok)`
3. server `output(output_seq=...)`

## 10.3 Sequence Gap Recovery

1. client sees unexpected `output_seq` gap (`coalesced=false`)
2. client `resync(reason=sequence_gap)`
3. server `ack(resync,ok)`
4. server `attached(initial_snapshot_ansi_base64,new_seq)`

---

## 11. Implementation Notes (Phase 21-A)

1. v2.0 initial implementation uses JSON for speed of delivery.
2. frame encode/decode must be reusable package (daemon+app tests share fixtures).
3. add replay fixtures for:
   - coalesced output
   - sequence gap
   - resize conflict
   - bridge restart
4. Phase 21-A で `bytes_base64` overhead（帯域/CPU）を必須計測し、閾値超過時は `binary_frames` を有効化する。

---

## 12. Validation Checklist

1. Parser rejects oversized frame.
2. Unknown `type` returns `error(e_protocol_invalid_frame)`.
3. `output_seq` monotonic under high throughput.
4. `coalesced=true` + seq jump では resync しないこと。
5. `focus` conflict policy deterministic.
6. reconnect + resync restores visible state.
