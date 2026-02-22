# AGTMUX Protocol v3 Wire Spec

Date: 2026-02-21  
Status: Active  
Depends on: `../20-unified-design.md`, `../30-detailed-design.md`

## 1. Purpose

v3 protocol の実装を迷わないため、wire format を固定する。

## 2. Transport

1. local: Unix Domain Socket
2. ssh: UDS relay（ADR-0002 により同一wireを透過）
3. frame境界: `u32be total_len` + `frame_bytes`

`total_len` は `header + payload` の合計バイト数。

## 3. Frame Envelope

`frame_bytes` は次で構成する。

1. fixed header（28 bytes, network byte order / big-endian）
2. payload（MessagePack, canonical map）

Header layout:

1. `magic` `[u8;4]` = `AGV3`
2. `version` `u8` = `1`
3. `frame_type` `u8`
4. `flags` `u16`
5. `request_id` `u32` (`0` は unsolicited event)
6. `stream_id` `u32` (pane stream / control stream 識別)
7. `sequence` `u64` (streamごと単調増加)
8. `payload_len` `u32` (payload bytes)

Validation rules:

1. `magic` 不一致は即切断
2. `version` 非対応は `ERR_UNSUPPORTED_VERSION`
3. `payload_len` と実長不一致は `ERR_BAD_FRAME`

## 4. Encoding Rules

1. payload は MessagePack map（string key）
2. 未知keyは無視（forward compatibility）
3. 必須key欠落は `ERR_BAD_PAYLOAD`
4. binary fields（terminal bytes）は `bin` を使う

## 5. Frame Type IDs

Command/Response:

1. `0x01` `hello`
2. `0x02` `hello_ack`
3. `0x03` `attach`
4. `0x04` `attached`
5. `0x05` `focus`
6. `0x06` `write`
7. `0x07` `resize`
8. `0x08` `ack`
9. `0x09` `error`
10. `0x0A` `layout_mutate`
11. `0x0B` `layout_preview`
12. `0x0C` `layout_commit`
13. `0x0D` `layout_revert`

Events:

1. `0x20` `topology_sync`
2. `0x21` `topology_delta`
3. `0x22` `output`
4. `0x23` `state`
5. `0x24` `metrics`
6. `0x25` `output_raw`（reserved, v3.1 optional）

## 6. Flags

1. `0x0001` `FLAG_COMPRESSED`（v3.1 reserved）
2. `0x0002` `FLAG_FINAL`（multi-part payload末尾）
3. `0x0004` `FLAG_ERROR`（error frame hint）

v3.0 では `FLAG_COMPRESSED` を使わない。

## 7. Required Payload Keys

`hello`:

1. `client_name` string
2. `client_version` string
3. `capabilities` array<string>

`attach`:

1. `target_id` string
2. `session_id` string
3. `window_id` string
4. `pane_id` string
5. `pane_epoch` uint

`write`:

1. `target_id` string
2. `session_id` string
3. `window_id` string
4. `pane_id` string
5. `pane_epoch` uint
6. `bytes` bin

`output`:

1. `target_id` string
2. `session_id` string
3. `window_id` string
4. `pane_id` string
5. `pane_epoch` uint
6. `source` enum(`pane_tap|bridge_fallback|preview`)
7. `bytes` bin
8. `seq` uint

`error`:

1. `code` string
2. `message` string
3. `retryable` bool
4. `details` map (optional)

## 8. Error Code Registry

1. `ERR_UNSUPPORTED_VERSION`
2. `ERR_BAD_FRAME`
3. `ERR_BAD_PAYLOAD`
4. `ERR_UNKNOWN_FRAME_TYPE`
5. `ERR_STALE_PANE`
6. `ERR_TARGET_DOWN`
7. `ERR_STREAM_NOT_FOUND`
8. `ERR_LAYOUT_LOCKED`
9. `ERR_LAYOUT_TIMEOUT`
10. `ERR_INTERNAL`

## 9. Ordering and Idempotency

1. client command は `request_id` 必須（`>0`）
2. server は同 `request_id` 再送を idempotent に処理
3. `output.seq` は stream_id ごと単調増加
4. `pane_epoch` 不一致 command は fail-closed (`ERR_STALE_PANE`)

## 10. Compatibility Policy

1. `version=1` は v2初期の固定wire
2. 新frame type追加は後方互換（既存type変更禁止）
3. 互換破壊は `version` を上げる
4. `output_raw` は `specs/76-output-hotpath-framing-policy.md` の trigger を満たした場合のみ導入する

## 11. Reference Tests (Mandatory)

1. header encode/decode roundtrip
2. payload validation tests（必須key欠落/型不一致）
3. frame length mismatch handling
4. stale pane rejection
5. unknown field tolerance
