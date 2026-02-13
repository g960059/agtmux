# AGTMUX v0.5 多角的レビュー 第2回 統合サマリー

**レビュー日**: 2026-02-13（修正反映後）
**レビュー体制**: Opus 4名による並列再レビュー

---

## 前回指摘の対応状況

| 判定 | 件数 | 説明 |
|------|------|------|
| **Resolved** | 30+ | daemon transport/lifecycle, SSH lifecycle, State Engine/Reconciler責務境界, runtime identity, ordering/dedupe, event_inbox binding, ゲート整合, CI基盤等 |
| **Partially Resolved** | ~10 | Watch バックプレッシャー, Reconciler負荷制御, CLI hook payload 未検証, Unit テスト不足, Pass Criteria 粒度, 境界値テスト等 |
| **Unresolved** | 3 | 技術スタック未選定, Property テスト対象, 開発リソース前提 |

**修正品質は高く、前回 Critical 指摘の大半が適切に解決された。**

---

## 残存 Critical 指摘（全レビュアー横断）

| ID | レビュアー | 概要 | Phase 0 ブロッカーか |
|---|---|---|---|
| R-01 (実現可能性) | **実装言語・フレームワーク未選定** | SQLiteバインディング、HTTP server、SSH client等の選択が全て言語依存 | **Yes** |
| R-07 (テスト) | **Adapter間相互作用テストの欠落** | 複数adapter並走時の一貫性テストが存在しない | No (Phase 2で顕在化) |

---

## 残存 Major 指摘（全レビュアー横断）

| ID | レビュアー | 概要 |
|---|---|---|
| N-02 (アーキ) | Watch Stream のバックプレッシャーポリシーが未定義（slow consumer対策） |
| N-04 (アーキ) | TCP listener 有効時の token 認証方式の詳細が未定義 |
| R-01 (アーキ) | State Engine内部のCAS/serialization方式が明示されていない |
| X-01 (アーキ) | Phase 1 での action 基盤の責務範囲が plan に未明記 |
| D-1 (整合性) | implementation-plan Phase 0 スキーマリストに `targets`, `panes` テーブルが欠落 |
| D-2 (整合性) | TASK-001 の FR参照に FR-11 欠如、adapters テーブルの責任タスクが曖昧 |
| R-02 (実現可能性) | Claude/Codex の実 hook ペイロードが未検証（フィクスチャが実CLI出力に基づくか不明） |
| R-03/N-03 (実現可能性) | CI環境での local sshd harness の実現性が未検証 |
| R-02 (テスト) | TASK-040 の Acceptance Criteria が「green の定義」を含まない |
| R-04 (テスト) | テストピラミッドが逆三角形（Unit 3.8%、E2E+Integration 58.5%） |
| R-05 (テスト) | TASK-040 と TASK-039 の TC-044 責任分界が不明確 |
| R-08 (テスト) | ネガティブ入力（不正JSON、不正ref、不正cursor等）のテストカタログが不在 |
| R-09 (テスト) | テスト並列実行時のリソース分離戦略（DB、UDS、SSH port）が不完全 |

---

## 横断分析：修正後の残存テーマ

### Theme 1: 実装言語・技術スタック未選定（Critical）
4名中3名が指摘。Phase 0 のコーディング開始に必須であり、最大のブロッカー。

**推奨**: Go / Rust / TypeScript の候補評価を即実施。評価観点:
- SQLite embedded support の成熟度
- UDS HTTP server の実装容易性
- SSH ControlMaster-equivalent の実現方法
- macOS app (Phase 3) との連携容易性

### Theme 2: CLI Hook/Notify の実ペイロード未検証（Major）
前回 Critical → 仕様上は解決（adapter contract 定義済み）だが、実際のCLI出力キャプチャが未実施。

**推奨**: Phase 0 完了前に Claude CLI hook と Codex notify の実出力をキャプチャし、test fixture として格納。

### Theme 3: テストピラミッドの逆転（Major）
Unit テスト 2件 (3.8%) は依然として低すぎる。以下を追加すべき:
1. State precedence 比較関数
2. `effective_event_time` 計算ロジック
3. `dedupe_key` 導出
4. `<ref>` BNF パーサー
5. `session-enc` percent-encoding/decoding
6. `pane_epoch` インクリメント判定
7. Target health state machine 遷移関数
8. Snapshot TTL 有効期限判定
9. Retention 期限計算
10. Adapter capability flag 解釈

### Theme 4: implementation-plan Phase 0 スキーマリストの不完全性（Major）
Plan の Phase 0 スキーマ列挙が `runtimes, events, event_inbox, runtime_source_cursors, states, actions, action_snapshots` のみで、`targets`, `panes` が欠落。TASK-031/032 で必要。

### Theme 5: Watch バックプレッシャー未定義（Major）
Cursor/resume/reset は定義済みだが、slow consumer への対処（バッファ上限、overflow時の切断/reset）が未規定。

---

## 特に評価された改善点

全レビュアーが以下を高く評価:

1. **State Ownership Boundary** (7.1.1) — Reconciler/State Engine の責務分離が明確化、前回最大リスクの解消
2. **SSH Lifecycle** (7.1.2) — timeout/retry/backoff の定量的定義、target health 遷移ルール
3. **agtmuxd API v1** (7.9) — UDS transport, lifecycle, health endpoint, error contract の normative 定義
4. **CI/Nightly 実行基盤** — TASK-040 + test-catalog Section 7 + implementation-plan Section 10
5. **フェーズゲート整合** — TC-046 の Phase 2 追加、TASK-023 の Phase 1 移動、TC-044 の TASK-039 紐付け

---

## Phase 0 開始前の推奨アクション（優先順）

| 優先度 | アクション | ブロッカー |
|---|---|---|
| **P0** | 実装言語・フレームワーク選定 | Yes — Phase 0 coding 開始不可 |
| **P0** | implementation-plan Phase 0 スキーマリストに `targets`, `panes` 追加 | Yes — TASK-001 の scope 不明確 |
| **P1** | Claude/Codex hook の実ペイロードキャプチャ・フィクスチャ化 | Phase 1 ブロッカー |
| **P1** | CI 環境 sshd harness の PoC | TASK-040 着手時に必要 |
| **P1** | Unit テストケース 10件の test-catalog 追加 | テストピラミッド是正 |
| **P2** | Watch stream バックプレッシャーポリシー定義 | Phase 1 close 推奨 |
| **P2** | State Engine の CAS/serialization 方式明記 | Phase 1 close 推奨 |
| **P2** | Phase 1 action 基盤の責務範囲を plan に明記 | 認識ブレ防止 |
| **P2** | health endpoint のレスポンススキーマ定義 | TC-040 green 判定に必要 |
| **P3** | TCP auth の token 管理仕様 | Phase 3 前に凍結 |
| **P3** | Adapter 間相互作用テスト設計 | Phase 2 前に準備 |

---

## 総合判定

**Go with minor changes** — 前回の Critical/Major が大半解決され、仕様の成熟度は大幅に向上。唯一の真のブロッカーは **実装言語の未選定**。これを解決すれば Phase 0 開始可能。
