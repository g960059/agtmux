# Phase 19: Evidence-Driven State Engine V2 設計計画 (2026-02-19)

## 1. 背景と課題

現行の status 判定は改善を重ねているが、以下の構造的課題が残る。

- `unknown + recent timestamp` 系の推定が provider ごとに揺れやすく、誤判定を誘発する。
- 判定ロジックが複数箇所に散在し、変更時の副作用範囲が読みにくい。
- `claude/codex/gemini/copilot` など provider 追加時に if/regex 追加が増え、保守コストが指数的に上がる。
- 判定理由 (`why running?`) を UI/運用で説明しづらい。
- Claude では `--resume` 非依存起動時に pane title が `session_name` へフォールバックしやすく、`claude /resume` 一覧のタイトルと一致しない。

このため、**推定中心の分岐ロジック**から、**証拠 (evidence) を合成して状態を決定するエンジン**へ置き換える。

## 2. ゴール / 非ゴール

### ゴール

- 判定責務を daemon 側へ一元化し、UI は表示専用にする。
- provider 追加時に `Adapter` 実装だけで拡張可能にする。
- `running / waiting_input / waiting_approval / idle / error / unmanaged / unknown` を一貫して判定できる。
- 判定に `source`, `confidence`, `reasons[]`, `evidence_trace` を持たせ、説明可能にする。
- 誤判定の再現データを replay テスト化し、回帰を防ぐ。
- Claude pane title は `/resume` 由来のセッション表示（first prompt / display）に高確率で一致させる。

### 非ゴール

- この phase で UI デザイン全面改修はしない。
- すべての provider の完全自動判定を一気に達成しない（段階導入）。
- tmux 非依存ランタイムへの全面移行はしない。

## 3. 設計原則

1. **Hook/Event First**: 明示イベントを最優先。
2. **Heuristic Last**: capture-pane 推定は最終 fallback。
3. **State Machine + Hysteresis**: 単発フレームでは遷移しない。
4. **Provider Isolation**: provider ごとの差分は adapter に閉じ込める。
5. **Explainability**: すべての状態に「根拠」を残す。
6. **Deterministic Replay**: ログから同じ結果を再現可能にする。

## 4. 全体アーキテクチャ

```text
[tmux control stream] ----\
[capture-pane snapshots] ---\
[hook events] ---------------> [Evidence Normalizer] -> [State Engine] -> [State Store] -> [API]
[wrapper lifecycle] ---------/           |                    |               |
[manual overrides] ---------/            -> [Provider Adapter]-+               -> [Metrics/Trace]
                                         -> [Session Identity Resolver]
```

### 4.1 コンポーネント

- `internal/stateengine/`
  - `engine.go`: evidence 合成・遷移。
  - `fsm.go`: 遷移ルール・ヒステリシス。
  - `score.go`: evidence スコア計算。
  - `types.go`: 共通ドメイン型。
- `internal/provideradapters/`
  - `claude/adapter.go`
  - `codex/adapter.go`
  - `gemini/adapter.go`
  - `copilot/adapter.go`
  - `registry.go`
- `internal/evidence/`
  - `normalizer.go`
  - `sources/` (hook, tmux_control, capture, wrapper)
- `internal/replay/`
  - fixture ロード・再生検証。
- `internal/sessionidentity/` (Phase 19-B で導入)
  - `claude_resolver.go`: `history.jsonl + projects/*.jsonl + runtime started_at` による runtime->session 推定。

## 5. ドメインモデル

### 5.1 基本型

- `AgentPresence`: `managed | unmanaged | unknown`
- `ActivityState`:
  - `running`
  - `waiting_input`
  - `waiting_approval`
  - `idle`
  - `error`
  - `unknown`
- `Provider`: `claude | codex | gemini | copilot | none | unknown`

### 5.2 Evidence

```go
type Evidence struct {
  PaneKey        PaneKey
  ProviderHint   string
  Kind           EvidenceKind   // hook, protocol, wrapper, tmux_control, capture
  Signal         SignalType     // running, waiting_input, waiting_approval, idle, error, none
  Weight         float64
  Confidence     float64
  Timestamp      time.Time
  TTL            time.Duration
  ReasonCode     string
  RawExcerpt     string
}
```

### 5.3 状態出力

```go
type PaneState struct {
  PaneKey           PaneKey
  Provider          string
  AgentPresence     string
  ActivityState     string
  Confidence        float64
  Source            string
  Reasons           []string
  UpdatedAt         time.Time
  LastInteractionAt *time.Time
  EvidenceTraceID   string
}
```

## 6. Provider Adapter 設計

```go
type ProviderAdapter interface {
  ID() string
  DetectProvider(meta PaneMeta, frame TerminalFrame) (provider string, confidence float64)
  ParseHookEvent(event RawHookEvent) []Evidence
  ParseControlFrame(frame TerminalFrame) []Evidence
  ParseCaptureSnapshot(snapshot SnapshotText) []Evidence
  ParseWrapperEvent(event WrapperEvent) []Evidence
  NormalizeSignal(e Evidence) Evidence
}
```

### 6.1 Adapter の責務

- provider 特有 UI 文字列・イベント語彙を Evidence に変換。
- provider 特有の優先順位（例: waiting > running）を signal 正規化に反映。
- 誤検知しやすい語彙（false-positive）を deny-list で吸収。
- provider 固有の session タイトル解決（例: Claude `/resume` 相当）を resolver へ委譲。

### 6.2 共通ルール（adapter 外）

- スコア合成
- 状態遷移
- TTL 失効
- confidence 閾値
- fallback ポリシー

## 7. 状態遷移 (FSM)

### 7.1 優先順位

- `error` > `waiting_approval` > `waiting_input` > `running` > `idle` > `unknown`

### 7.2 遷移条件

- `unknown -> running`
  - 明示 running signal が一定スコア超過時のみ。
- `running -> idle`
  - idle/completed signal 継続 + running evidence 失効時。
- `running -> waiting_*`
  - waiting signal が 1 回でも高信頼なら即遷移。
- `waiting_* -> idle`
  - user action event または waiting evidence TTL 失効後。

### 7.3 ヒステリシス

- `enter_threshold` と `exit_threshold` を分離。
- `min_stable_duration` を state ごとに設定。
- chattering 防止のため `cooldown_window` を導入。

## 8. Evidence ソース優先度

1. Hook / Protocol (weight 1.0)
2. Wrapper lifecycle (0.9)
3. tmux control-mode frame (0.7)
4. capture-pane heuristic (0.5)
5. stale fallback (0.2)

同一時刻帯で矛盾がある場合は、`weight * confidence` 最大を採用。

## 9. API 変更計画

`view panes` 応答に以下を追加/標準化。

- `activity_state`
- `activity_confidence`
- `activity_source`
- `activity_reasons[]`
- `evidence_trace_id`
- `provider_confidence`

オプションで debug endpoint。

- `GET /debug/panes/:pane/evidence?limit=100`

## 10. データ保存

SQLite に新テーブル追加。

- `pane_state_v2`
- `pane_evidence_log`
- `provider_detection_log`

保持ポリシー:

- evidence raw は 24h (ring buffer)
- state snapshot は 7d

## 11. 段階的移行戦略

### Phase 19-A: 土台

- `stateengine` と `provideradapter` skeleton 作成。
- 既存レスポンスと並行して `v2_state` を計算（shadow mode）。

### Phase 19-B: Claude adapter

- hook/control/capture の 3入力を Claude 用に統合。
- **Claude Session Identity V2**
  - `--resume` がある場合: session id を最優先採用。
  - `--resume` がない場合: `~/.claude/history.jsonl`（project/display/timestamp）と
    `~/.claude/projects/<project>/*.jsonl` を統合し、`runtime.started_at` 近傍で割当。
  - 複数 Claude runtime が同一 workspace にいる場合も、同一 title 重複を最小化。
- 既知誤判定 fixture を replay 化（`claude idle but running`, `claude title mismatch`）。

### Phase 19-C: Codex adapter

- Codex running/idle/waiting 信号を明示化。
- Claude 向け UI contrast 判定との境界を provider confidence で分離。

### Phase 19-D: Gemini/Copilot adapter

- 最小実装 + fallback。
- provider 未確定時は `unknown` 維持（無理に running にしない）。

### Phase 19-E: UI切替

- AppViewModel の推定ロジックを削除。
- daemon の `activity_state` をそのまま表示。

### Phase 19-F: observability

- 指標追加: `misclass_feedback`, `unknown_rate`, `state_flip_rate`, `decision_latency_ms`。
- 指標追加: `claude_title_match_rate`, `claude_title_source_distribution`.

### Phase 19-G: 旧ロジック削除

- v1 判定関数廃止。
- API v2 を default に昇格。

## 12. TDD / テスト戦略

### 12.1 Unit

- adapter 単体: signal 抽出、false-positive 抑制。
- FSM 単体: 遷移条件、ヒステリシス。
- score 単体: weight/confidence/TTL。

### 12.2 Replay

- 実ログ fixture から deterministic に state 列を再現。
- `claude idle but running` 既知事象を必須 fixture 化。
- `codex pane に claude contrast 誤適用` を provider 判定 fixture 化。

### 12.3 Integration

- daemon endpoint で state/evidence consistency を検証。
- target 複数時に pane key 混線しないことを検証。

### 12.4 Performance

- 1 tick あたり state 計算 budget: <= 5ms / 100 panes。
- evidence log 書き込みで UI 更新を阻害しない。

## 13. 受け入れ条件 (AC)

1. `unknown + recent` だけで `running` にならない。
2. `claude` idle の誤判定率が現行比 70% 以上改善。
3. provider 増設時に既存 adapter 変更なしで追加可能。
4. すべての pane state に `source` と `reasons[]` が付与される。
5. replay test で主要 fixture 全 pass。
6. Claude pane title の 80% 以上が `session_name` fallback ではなく
   `claude_session_jsonl | claude_history_display` 由来になる。

## 14. リスクと対策

- リスク: provider UI 更新で regex 崩壊。
  - 対策: regex 依存を adapter 局所化 + fixture 更新手順を標準化。
- リスク: evidence 増大で負荷増。
  - 対策: TTL + ring buffer + batch insert。
- リスク: hook 未導入環境で精度不足。
  - 対策: wrapper event と control-mode を第二優先に。
- リスク: `history.jsonl` が巨大化し、毎 tick で読み込むと遅延。
  - 対策: mtime ベース cache と workspace 上位 N 候補のみ解決。

## 15. 実装着手時の優先タスク (実行順)

1. `internal/stateengine/types.go` と `engine.go` を追加。
2. `provideradapters/registry.go` と `claude/adapter.go` を追加。
3. daemon の snapshot 経路に `stateengine.Evaluate()` を差し込む（shadow mode）。
4. replay harness を追加し、既知不具合 fixture を登録。
5. `AppViewModel` の判定 fallback を feature flag 経由で段階的に無効化。
6. Claude Session Identity V2 を daemon に組み込み、`session_label_src` を
   `claude_session_jsonl | claude_history_display | claude_resume_id` で可観測化。

## 17. 外部実装サーベイ（Claude タイトル/状態）

- `tmux-claude-status`:
  - hook ファーストで status を確定（`UserPromptSubmit/PreToolUse/Stop/Notification`）。
  - タイトル推定は扱わない。状態精度を hooks で担保する思想。
- `agent-viewer`:
  - pane テキストのヒューリスティック判定中心（running/idle/completed）。
  - タイトルは LLM ラベリングで補完。`/resume` 一致は設計対象外。
- `agent-session-manager`:
  - `~/.claude/history.jsonl` と `~/.claude/projects/*.jsonl` を主情報源として
    session list/title を構築。Claude `/resume` 互換性を重視。

本プロジェクトは Phase 19-B で `agent-session-manager` 系のデータソース設計を取り込み、
status は hook/event、title は history/session 統合推定で分離して精度を上げる。

## 16. 完了判定

- shadow mode と current mode の差分レポートが取得できる。
- 既知不具合（Claude idle 誤 running、Codex/Claude 誤判定）で regression なし。
- UI 側に判定ロジックが残っていない。
