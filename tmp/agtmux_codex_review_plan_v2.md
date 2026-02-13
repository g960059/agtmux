`docs/agtmux-spec.md` の再レビュー結果です（ロードマップ/phase/実行可能性/MVP依存/スコープ/app移行観点）。

**Critical**
1. `runtime_id`/`pane_epoch` の生成・更新規則が未定義で、誤ペイン操作リスクが残っています。  
根拠: `docs/agtmux-spec.md:79`, `docs/agtmux-spec.md:209`, `docs/agtmux-spec.md:214`, `docs/agtmux-spec.md:370`  
改善案: `pane_epoch` の増分条件（PID変化、pane再作成、observer再同期）を明文化し、`send/kill` は `runtime_id + state_version` 完全一致を必須化。加えて「古いruntimeへの操作拒否」を受け入れテストに固定。

2. イベント順序保証が分散時刻ズレ前提で不足しています。  
根拠: `docs/agtmux-spec.md:80`, `docs/agtmux-spec.md:221`, `docs/agtmux-spec.md:249`, `docs/agtmux-spec.md:154`  
改善案: 適用順序を仕様化（`source_seq`優先、次に`event_time`、同値時`ingested_at,event_id`）し、clock skew許容幅を定義。`source_seq`欠損時の決定的ルールを追加。

**High**
3. Phase 0 が重すぎて「最初の価値到達」が遅延しやすいです。  
根拠: `docs/agtmux-spec.md:344`, `docs/agtmux-spec.md:350`  
改善案: Phase 0 を「0a:単一target可視化」「0b:安全性コア」に分割。0aで早期に`list panes`可動を作る。

4. MVP成立条件が定性的で、完了判定がブレます。  
根拠: `docs/agtmux-spec.md:365`, `docs/agtmux-spec.md:366`, `docs/agtmux-spec.md:85`  
改善案: 指標化（例: 更新遅延P95<=2s、`waiting_*`誤判定率<=5%、target 1台ダウン時でも一覧応答<=3s）。

5. Control MVPの依存ゲートが不足しています。  
根拠: `docs/agtmux-spec.md:370`, `docs/agtmux-spec.md:375`, `docs/agtmux-spec.md:81`  
改善案: Phase 1.5 の前提に「runtime guardプロパティテスト100%」「stale action拒否E2E」を追加。

6. app移行の要となる daemon API 契約が未凍結です。  
根拠: `docs/agtmux-spec.md:109`, `docs/agtmux-spec.md:330`, `docs/agtmux-spec.md:295`  
改善案: `agtmuxd API v1` を Phase 2 より前に定義（バージョニング、互換ポリシー、ストリーミング仕様）。

7. `events` 増大時の性能・保持ポリシーが未定義です。  
根拠: `docs/agtmux-spec.md:215`, `docs/agtmux-spec.md:226`, `docs/agtmux-spec.md:85`  
改善案: TTL/アーカイブ方針とインデックスを明記。NFR-1を運用で維持できる形にする。

**Medium**
8. Copilot/Cursor追加が app 前に来ており、スコープ肥大しやすいです。  
根拠: `docs/agtmux-spec.md:385`, `docs/agtmux-spec.md:395`  
改善案: 2.5を app後ろへ移動、または「1アダプタのみ実証」に縮小。

9. 既定表示（`--all-targets`）が未確定で、大規模時のUX/性能に影響します。  
根拠: `docs/agtmux-spec.md:282`, `docs/agtmux-spec.md:432`  
改善案: MVP中に固定（推奨: デフォルト current-target、`--all-targets`明示）し、将来フラグで切替。

10. target接続の認証/権限境界が不足しています。  
根拠: `docs/agtmux-spec.md:192`, `docs/agtmux-spec.md:277`  
改善案: SSH鍵管理、許可コマンド、監査ログ最小要件を追記。

11. Adapter互換性要件(NFR-6)の検証機構がありません。  
根拠: `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:167`  
改善案: contract test suite と `ContractVersion` 互換ルール（semver）を phase exit criteria 化。

**Low**
12. 「Visibility MVP」と「Control MVP(1.5)」の名称が曖昧で、社内認識がずれやすいです。  
根拠: `docs/agtmux-spec.md:356`, `docs/agtmux-spec.md:368`  
改善案: MVPを1つに定義し、`MVP-Read`/`MVP-Write`のように明示分離。

13. リスク章に運用監視（SLO/アラート）が不足しています。  
根拠: `docs/agtmux-spec.md:404`  
改善案: 遅延・unknown率・target失敗率の監視項目を追加。

---

**Phase再編提案（推奨）**
1. Phase 0a: Local single-target visibility skeleton（`list panes` + SQLite最小 + observer）。  
2. Phase 0b: Safety core（runtime identity, dedupe/order, reconciler, stale demotion）。  
3. Phase 1: Visibility MVP（Claude/Codex + multi-target + sessions/windows + watch）。  
4. Phase 1.5: Control MVP（send/view-output/kill + fail-closed + audit）。  
5. Phase 2: API v1 freeze + Gemini（appが直接使う契約を確定）。  
6. Phase 3: macOS app（API v1のみ利用）。  
7. Phase 4: Adapter expansion（Copilot/Cursor）。

---

**反映推奨 TOP5**
1. `runtime_id/pane_epoch` の厳密仕様化と stale-action拒否テスト追加。  
2. イベント順序アルゴリズム（clock skew含む）を仕様へ固定。  
3. Phase 0 を 0a/0b に分割し、早期価値提供を先行。  
4. 各PhaseのExit Criteriaを定量化（遅延・誤判定・部分障害耐性）。  
5. `agtmuxd API v1` を app前に凍結（version/compat/streaming定義）。

---

**そのまま追記できる修正文案（例）**
```md
### 7.2.3 Runtime Identity Rules
- `pane_epoch` is incremented when pane process identity changes (`pane_pid` change, pane recreation, or observer resync with non-matching runtime).
- `runtime_id` = sha256(target_id + pane_id + pane_epoch + agent_type + started_at_ns).
- Mutating actions (`send`, `kill`) MUST include `if-runtime` and MUST be rejected unless `(runtime_id,state_version)` matches current state row.
- Reconciler MUST emit runtime-end tombstone before opening a new runtime on same pane.
```

```go
// Deterministic event ordering (spec reference implementation)
func EventLess(a, b Event) bool {
    if a.SourceSeq != nil && b.SourceSeq != nil && a.RuntimeID == b.RuntimeID {
        return *a.SourceSeq < *b.SourceSeq
    }
    if !a.EventTime.Equal(b.EventTime) {
        return a.EventTime.Before(b.EventTime)
    }
    if !a.IngestedAt.Equal(b.IngestedAt) {
        return a.IngestedAt.Before(b.IngestedAt)
    }
    return a.EventID < b.EventID
}
```

```sql
-- Performance guardrails for NFR-1
CREATE INDEX IF NOT EXISTS idx_events_runtime_ingested
ON events(runtime_id, ingested_at DESC);

CREATE INDEX IF NOT EXISTS idx_states_updated
ON states(updated_at DESC);

-- Retention policy (example): keep raw events for 14 days
DELETE FROM events WHERE ingested_at < datetime('now', '-14 days');
```