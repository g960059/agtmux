**指摘（重大度順）**

1. **[Critical] clock skew超過時に古いイベントが「最新」として適用される**
参照: `docs/agtmux-spec.md:176` `docs/agtmux-spec.md:177` `docs/agtmux-spec.md:367`  
`effective_event_time = ingested_at` は、遅延到着した古いイベントを新規イベント扱いにしてしまいます。`unknown/stale` を誤って `running/waiting_*` に戻すリスクがあります。  
改善案: skew超過イベントは「保存はするが state/cursor は更新しない」に変更し、`reason_code=clock_skew_exceeded` を付与。

```go
func classifyOrder(e Event, offset time.Duration, budget time.Duration) (OrderKey, bool, string) {
    if e.SourceSeq != nil {
        return OrderKey{Seq: *e.SourceSeq, IngestedAt: e.IngestedAt, EventID: e.EventID}, true, ""
    }
    corrected := e.EventTime.Add(-offset) // target clock offset補正
    if absDuration(corrected.Sub(e.IngestedAt)) > budget {
        return OrderKey{}, false, "clock_skew_exceeded" // 保存のみ
    }
    return OrderKey{EventTime: corrected, IngestedAt: e.IngestedAt, EventID: e.EventID}, true, ""
}
```

2. **[Critical] 「paneごとにactive runtimeは1つ」の不変条件がDBで強制されていない**
参照: `docs/agtmux-spec.md:216` `docs/agtmux-spec.md:248`  
仕様上は禁止でも、DB制約がないため race で複数 active runtime が入り得ます。`bind_ambiguous` や誤った action 宛先の直接原因になります。  
改善案: partial unique index で強制。あわせて action/event の runtime FK を明示。

```sql
CREATE UNIQUE INDEX IF NOT EXISTS uq_runtimes_active_per_pane
ON runtimes(target_id, pane_id)
WHERE ended_at IS NULL;
```

3. **[High] `event_inbox` の dedupe キーが runtime世代をまたいで衝突する**
参照: `docs/agtmux-spec.md:277` `docs/agtmux-spec.md:370`  
`UNIQUE(target_id,pane_id,source,dedupe_key)` だと pane再利用後の正当イベントが重複扱いで落ちます。  
改善案: `bound` と `pending_bind` で dedupe 制約を分離。

```sql
CREATE UNIQUE INDEX IF NOT EXISTS uq_event_inbox_bound
ON event_inbox(runtime_id, source, dedupe_key)
WHERE runtime_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_event_inbox_pending
ON event_inbox(target_id, pane_id, source, dedupe_key, COALESCE(pid,-1), COALESCE(start_hint,''))
WHERE runtime_id IS NULL;
```

4. **[High] dedupe判定・cursor更新・state更新のトランザクション境界が未規定**
参照: `docs/agtmux-spec.md:171` `docs/agtmux-spec.md:181` `docs/agtmux-spec.md:285`  
並行 ingest 時に lost update が起き、順序保証が壊れる可能性があります。  
改善案: 1イベント適用を単一トランザクションで規定。

```sql
BEGIN IMMEDIATE;
-- 1) inbox insert (ON CONFLICT DO NOTHING)
-- 2) runtime bind
-- 3) cursor compare
-- 4) events insert
-- 5) runtime_source_cursors upsert
-- 6) states upsert (state_version = state_version + 1)
COMMIT;
```

5. **[High] `actions.request_ref` の冪等性契約が弱く、再送で二重実行し得る**
参照: `docs/agtmux-spec.md:311` `docs/agtmux-spec.md:539`  
ネットワークリトライ時に `send/kill` が重複発火する危険があります。  
改善案: `request_ref` を必須UUIDにし、ユニーク制約+再送時は既存 action を返す。

```sql
CREATE UNIQUE INDEX IF NOT EXISTS uq_actions_request_ref
ON actions(action_type, request_ref);
```

```sql
INSERT INTO actions(action_id, action_type, request_ref, target_id, pane_id, runtime_id, requested_at, result_code)
VALUES (?, ?, ?, ?, ?, ?, ?, 'accepted')
ON CONFLICT(action_type, request_ref) DO NOTHING;
```

6. **[Medium] `pending_bind` の運用フィールド不足で滞留・障害解析が難しい**
参照: `docs/agtmux-spec.md:274` `docs/agtmux-spec.md:378`  
TTLの概念はあるが、`expires_at`/`attempt_count`/`next_retry_at` がなく、再試行戦略を実装しにくいです。  
改善案: `event_inbox` に `first_seen_at`, `expires_at`, `attempt_count`, `next_retry_at`, `bound_at` を追加し、`status,next_retry_at` インデックスを必須化。

7. **[Medium] `watch` の `sequence` 再起動耐性が未定義**
参照: `docs/agtmux-spec.md:460` `docs/agtmux-spec.md:466`  
daemon再起動で sequence が巻き戻ると、`--since` の再開が不正になります。  
改善案: `stream_epoch` を追加し `(stream_epoch, sequence)` をカーソルにする。`stream_epoch` は再起動時にインクリメントして永続化。

8. **[Medium] retention/index が `events/states` 偏重で、運用DB肥大化リスク**
参照: `docs/agtmux-spec.md:333` `docs/agtmux-spec.md:336`  
`event_inbox/actions/action_snapshots/runtime_source_cursors` のTTLと索引が未定義です。  
改善案: それぞれに retention と必須 index を追加。特に `event_inbox(status,next_retry_at)`、`actions(requested_at)`、`action_snapshots(action_id)` は最低限必要です。

---

**仕様追記の最小差分案（そのまま追記可能）**

```txt
- Skew超過イベント（source_seqなし）は state/cursor を更新してはならない。保存時は reason_code=clock_skew_exceeded。
- runtimes は (target_id, pane_id) WHERE ended_at IS NULL の一意制約を必須とする。
- event_inbox dedupe は bound/pending で別制約を持つ。
- ingestは単一DBトランザクションで dedupe->bind->order->persist->cursor->state を原子的に実行する。
- actions.request_ref は必須で、同一(action_type, request_ref)は同一action_idを返す。
- watchカーソルは (stream_epoch, sequence) とする。
```

**優先実装順（短期）**

1. skew超過の適用禁止 + active runtime 一意制約。  
2. `event_inbox` dedupe 分離 + ingestトランザクション規定。  
3. action冪等性 + pending_bind運用列 + watch epoch。
