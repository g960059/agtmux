# Output Hotpath Framing Policy

Date: 2026-02-21  
Status: Active  
Depends on: `70-protocol-v3-wire-spec.md`, `71-quality-gates.md`

## 1. Purpose

`output` フレームの MessagePack 維持か、binary 専用フレーム導入かを「実測」で判断する。

## 2. Default Policy

v3.0 初期実装は次を採用する。

1. すべて MessagePack payload
2. `output` も `bin` field で運ぶ
3. 最適化は SLO 未達時のみ実施

## 3. Measurement Protocol

以下を同一環境で 3 run 実施し median を採用する。

1. local: codex/claude の interactive trace replay
2. sustained output: 1MB/s, 5MB/s, 10MB/s
3. multi-pane: selected 1 + background 5

測定値:

1. `selected_stream_gap_p95_ms`
2. `active_fps_median`
3. desktop CPU%
4. daemon CPU%
5. encode/decode time per output frame

## 4. Escalation Trigger

次のいずれかで binary 専用フレーム導入を検討する。

1. `selected_stream_gap_p95_ms >= 40` が 2 run 以上
2. `active_fps_median < 55` が 2 run 以上
3. MessagePack encode+decode が output path CPU の 20% 超

## 5. Binary Frame Option (v3.1)

導入時の要件:

1. 新 frame type（例: `output_raw`）を追加
2. topology/state/ack は既存 MessagePack 維持
3. fallback path を持ち、feature flag で切替可能
4. wire version 互換を壊さない（後方互換追加のみ）

## 6. Documentation Gate

binary 導入前に必須:

1. ADR を追加
2. `70-protocol-v3-wire-spec.md` を更新
3. 回帰計測結果を `docs/v2/implementation-records` に保存
