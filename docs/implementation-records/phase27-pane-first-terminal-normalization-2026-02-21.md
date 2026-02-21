# Phase 27: Pane-first Terminal Normalization (2026-02-21)

Date: 2026-02-21
Status: In Progress (27-D completed)

Related:
- `docs/implementation-records/phase21-tty-v2-terminal-engine-detailed-design-2026-02-20.md`
- `docs/implementation-records/phase22-26-implementation-design-no-orchestration-2026-02-20.md`

## 0. Executive Summary

現状の不具合（snapshot mode 固定、二重文字、カーソルずれ、復旧時の表示崩れ）は、
「描画経路が複数ある」ことが主因。

Phase 27 では後方互換を前提にせず、selected pane の表示経路を 1 本に統一する。

- **採用方針**: `tmux pane first` を徹底
- **正規経路**: `tmux pane raw bytes -> tty-v2 output -> SwiftTerm.feed(bytes)`
- **廃止対象（selected pane）**: snapshot 混在、カーソル推定、local echo

## 1. Root Cause (現状課題)

1. snapshot と stream の混在
- VT state (alternate screen, scroll region, cursor save/restore) を壊す。

2. cursor 推定ロジックの残存
- CJK 幅/IME/alternate screen で必ずズレる。

3. local echo + stream echo の二重経路
- 入力文字が二重表示される。

4. bridge 可視性/接続条件への依存
- `%output` が来ないと空画面化しやすい。

## 2. Goal / Non-goal

## 2.1 Goal

1. selected pane の描画を raw stream only にする。
2. snapshot を selected pane hot path から排除する。
3. カーソル位置の真実源を SwiftTerm のみにする。
4. 入力を single-path 化し、二重表示を防止する。

## 2.2 Non-goal

1. daemon 側 VT エミュレータ実装。
2. orchestration 機能追加。
3. 全 UI の再設計。

## 3. Architecture (Phase 27)

### 3.1 Data Plane

- 新規: `PaneTapManager`（daemon）
- 方式: `tmux pipe-pane -O -t <pane>` を使い、対象 pane の raw bytes を 1 本で取得。
- selected pane 表示中は `pane_tap` を優先。

```
selected pane
  -> PaneTapManager.attach(pane)
  -> tmux pipe-pane -O
  -> raw bytes stream (single source of truth)
  -> tty-v2 output(source=pane_tap)
  -> SwiftTerm.feed(bytes)
```

### 3.2 Recovery Policy

- `stream_live` / `stream_recovering` の 2 状態。
- `stream_recovering` で一定時間 bytes が来ない場合は「recovering 表示」のみ。
- selected pane では snapshot fallback を使わない（混在禁止）。

### 3.3 Input Policy

- local echo 禁止。
- write ack は入力受理確認のみ。描画更新は stream bytes のみ。
- IME は AppKit marked text 標準経路に統一。

## 4. Implementation Plan

## 4.1 Phase 27-A (this turn: start)

1. design doc 追加（本ドキュメント）。
2. daemon に `PaneTapManager` 骨格を追加。
3. `tty_v2` に PaneTap attach/detach のフックを追加（feature flag guarded）。

## 4.2 Phase 27-B

1. selected pane output を `pane_tap` 優先に切り替え。
2. snapshot source を selected pane で無効化。
3. telemetry 追加:
   - `tty_output_source{source=pane_tap|bridge|snapshot}`
   - `tty_hotpath_capture_count(selected=true)`

## 4.3 Phase 27-C

1. App side の snapshot fallback を selected pane で無効化。
2. cursor 推定ロジック削除。
3. `stream_recovering -> stream_live` 遷移条件を bytes first に固定。

## 4.4 Phase 27-D

1. local echo パス削除。
2. input batching は維持しつつ描画は stream のみ。

## 4.5 Phase 27-E

1. IME marked text の最終統合。
2. 未確定文字と placeholder 重なりの再設計。

## 4.6 Phase 27-F

1. bridge 依存経路の縮退（meta 用途のみに限定）。
2. code cleanup（snapshot/cursor 推定 dead path 削除）。

## 5. Acceptance Criteria

1. selected pane で `snapshot` source が 0。
2. selected pane で `tty_hotpath_capture_count(selected=true)==0`。
3. 二重文字再現ケースで再発 0。
4. cursor ずれ既知ケース（CJK/Claude/Codex）で再発 0。
5. input p95 < 20ms (local), stream p50 <= 50ms を目標。

## 6. Risks / Mitigations

1. `pipe-pane` 競合
- Mitigation: 競合検出時に明示 error と fallback 導線を表示。

2. attach 初回空白
- Mitigation: `pipe ready -> focus -> resize pulse(SIGWINCH)` の順序保証。

3. non-TUI の過去出力可視性
- Mitigation: live-first を原則とし、必要なら history viewer を別導線化。

## 7. Rollout Strategy

1. `EnableTTYV2PaneTap` feature flag で段階有効化。
2. local target で先行適用。
3. telemetry が安定したら default-on。
4. 最後に snapshot fallback を selected pane から完全削除。

## 8. Progress (2026-02-21)

Completed in this phase start:

1. `EnableTTYV2PaneTap` config/daemon flag を追加。
2. daemon に `pane_tap.go` を追加（FIFO + `tmux pipe-pane -O` + lifecycle）。
3. `tty_v2.go` に pane tap attach/detach/focus を追加し、selected pane で `pane_tap` source を優先。
4. bridge output と pane tap output の二重送信を防ぐため、pane tap active 時は bridge output を suppress（layout イベントは継続）。
5. telemetry に `OutputPaneTap` を追加。
6. mac app attach を `want_initial_snapshot=false` に変更（selected pane stream-only）。
7. mac app 側で snapshot-like source の適用を停止（fallback 適用を無効化）。
8. `EnableTTYV2PaneTap` default を `true` に変更。
9. mac app の tty-v2 render mode を 2状態へ簡素化（`stream_live` / `stream_recovering`）。
10. snapshot fallback 用 state/task (`ttyV2PendingSnapshotByPaneID`, `ttyV2RecoveryTaskByPaneID`) を削除。

Phase 27-D (NativeTmuxTerminalView snapshot path removal):

11. `vtStreamMode` プロパティを `NativeTmuxTerminalView` struct / Coordinator / AGTMUXDesktopApp から削除。
12. Coordinator の render pipeline を VT stream 単一経路に統合（`renderIfNeeded` 1本化）。
13. snapshot 前提の描画関数をすべて削除:
    - `buildAbsoluteRepaintFrame`, `buildCursorOnlyFrame`, `RepaintFrame` struct
    - `inferCursorRow`, `inferCursorColumn`, `visibleColumnCount`
    - `updatedCachedLines`, `splitLines`
    - `normalizedTerminalText`, `applyClaudePromptTryLeadingGlyphHighlight`
    - `firstPromptInputCharacterRange`, `visibleCharacterRange`
    - `stripANSI`, `consumeEscapeSequence`, `clampLine`
    - `isSnapshotLikeVTSource`, `appendSnapshotCursorCSIIfNeeded`
    - `preparedVTFeedContent`, `shouldResetForVTStream` (inlined)
14. dead stored properties 削除: `lastRenderedLines`, `maxCachedLines`, `pendingVTStreamMode`, `currentVTStreamMode`
15. `terminalUsesVTStreamCursor` computed property を AppViewModel から削除。
16. ファイル行数: 1483 → 917 行（566 行削除 = 38% 削減）。

Validation:

1. `go test ./internal/config ./internal/daemon ./internal/ttyv2` PASS
2. `swift build` PASS
3. `swift test` 93 tests, 0 failures
