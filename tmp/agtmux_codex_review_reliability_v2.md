使用スキル: `codex`（レビュー依頼に該当）

**Findings**
1. **Critical** `send/kill` が「fail-closed」になっていません。  
根拠: `docs/agtmux-spec.md:81`, `docs/agtmux-spec.md:287`, `docs/agtmux-spec.md:289`, `docs/agtmux-spec.md:316`, `docs/agtmux-spec.md:326`  
再現（競合+切断）: `list` 取得後に pane が再生成（`runtime_id` 変更）され、ユーザーが `--if-runtime` なしで `kill` 実行すると新 runtime を誤って停止。  
具体改善案: 破壊的操作は `action_token` 必須に変更し、サーバー側で `runtime_id + state_version + expires_at` を原子的に検証。不一致は常に `PRECONDITION_FAILED`。

2. **Critical** dedupe 契約が未確定で、重複イベントを取りこぼします。  
根拠: `docs/agtmux-spec.md:252`, `docs/agtmux-spec.md:253`, `docs/agtmux-spec.md:224`, `docs/agtmux-spec.md:226`  
再現（逆順イベント）: adapter retry で同一 `source_event_id` が複数回来るが `dedupe_key` 未設定/不安定だと重複適用。  
具体改善案: `dedupe_key` を `NOT NULL` で必須化し、生成式を仕様固定（例: 正規化 payload を含む SHA-256）。

3. **High** ordering が source 跨ぎで非決定です。  
根拠: `docs/agtmux-spec.md:80`, `docs/agtmux-spec.md:92`, `docs/agtmux-spec.md:221`, `docs/agtmux-spec.md:235`  
再現（逆順イベント）: `notify(seq=120)` 後に `poller(seq=7)` が到着。`last_source_seq` 単一カラムだと poller 系が永続的に棄却され、収束しない。  
具体改善案: `runtime_id + source` 単位の cursor 表を追加し、順序判定を source ごとに管理。

4. **High** reconciler / ingest / demotion の競合時に収束保証が不足しています。  
根拠: `docs/agtmux-spec.md:269`, `docs/agtmux-spec.md:272`, `docs/agtmux-spec.md:162`, `docs/agtmux-spec.md:234`  
再現（競合）: `waiting_input` 更新と `completed->idle` demotion が同時実行され、後勝ちで `idle` が残る。  
具体改善案: すべての state 更新を CAS（`runtime_id + state_version`）必須に統一し、`RowsAffected=0` は再評価リトライ。

5. **High** 切断時の state convergence ルールが定量化されていません。  
根拠: `docs/agtmux-spec.md:270`, `docs/agtmux-spec.md:272`, `docs/agtmux-spec.md:411`, `docs/agtmux-spec.md:155`  
再現（切断）: target が 30 秒断で `running` が表示残存し、誤操作リスクが続く。  
具体改善案: TTL マトリクスを仕様化（例: 6s で `unknown/stale_signal`, 30s で `unknown/target_unreachable`）し、`down` 時は `send/kill` を既定拒否。

6. **Medium** `completed->idle` の基準時刻が曖昧で、時計ずれに弱いです。  
根拠: `docs/agtmux-spec.md:149`, `docs/agtmux-spec.md:254`, `docs/agtmux-spec.md:423`  
再現（逆順+時計ずれ）: remote `event_time` が未来/過去だと demotion が即時発火または過遅延。  
具体改善案: demotion 判定は daemon の `ingested_at` 基準に固定し、remote `event_time` は表示用途のみ。

7. **Low** `attach` だけ「should validate」で安全ポリシーが弱いです。  
根拠: `docs/agtmux-spec.md:317`, `docs/agtmux-spec.md:319`  
再現（切断復帰）: reconnect 後に pane 再利用済みでも stale 参照へ attach。  
具体改善案: `attach` も既定で precondition 必須に統一し、`--force` でのみ回避可。

**仕様追記の具体案（抜粋）**
```sql
-- 1) dedupe と ordering
CREATE TABLE runtime_source_cursor (
  runtime_id TEXT NOT NULL,
  source TEXT NOT NULL,
  last_source_seq INTEGER,
  last_ingested_at TEXT NOT NULL,
  PRIMARY KEY (runtime_id, source)
);

-- events.dedupe_key は必須
-- UNIQUE(runtime_id, source, dedupe_key)
```

```go
// 2) 単一路径の適用ロジック（ingest/reconciler/demotion 共通）
func applyTransition(tx *sql.Tx, t Transition) error {
  if !acceptByDedupeAndCursor(tx, t.Event) { return nil }
  st := loadState(tx, t.TargetID, t.PaneID)
  if st.RuntimeID != t.RuntimeID { return nil } // runtime guard
  if !isFresh(t.Event.IngestedAt, st) { return nil }

  res, err := tx.Exec(`
    UPDATE states
    SET state=?, reason_code=?, updated_at=?, state_version=state_version+1
    WHERE target_id=? AND pane_id=? AND runtime_id=? AND state_version=?`,
    t.NextState, t.ReasonCode, nowUTC(),
    st.TargetID, st.PaneID, st.RuntimeID, st.StateVersion)
  if err != nil { return err }
  if rowsAffected(res) == 0 { return ErrRetry }
  return nil
}
```

```go
// 3) fail-closed action
func validateAndExecuteKill(tx *sql.Tx, token ActionToken, ref PaneRef) error {
  st := loadState(tx, ref.TargetID, ref.PaneID)
  if token.Expired() || st.RuntimeID != token.RuntimeID || st.StateVersion != token.StateVersion {
    return ErrPreconditionFailed
  }
  return execTmuxKill(ref, "INT")
}
```

**再現シナリオ（要求3種）**
1. 競合: `completed` 到着直後に demotion ジョブが走り、次の `waiting_input` と競合して不正に `idle` 固定。  
2. 逆順イベント: `notify(seq=11)` が先に適用後、遅延した `notify(seq=10)` と `poller(no-seq)` が到着して実装依存で状態が分岐。  
3. 切断: target 切断中に pane が再生成され、復帰後 stale 参照への `kill/send` が通る。

**反映推奨 TOP5**
1. 破壊的操作を `action_token` 必須にして fail-closed を既定化。  
2. `dedupe_key` 必須化と生成式の仕様固定。  
3. `runtime_id+source` cursor 導入で ordering を決定化。  
4. CAS 更新（`runtime_id+state_version`）を全更新経路で強制。  
5. 切断時 TTL/health/action-block の定量ルールを明文化。