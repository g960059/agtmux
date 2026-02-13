**判定**  
`Go with changes`

**重大リスク一覧（Severity順）**
1. `Critical` アクションAPIに冪等性契約が未固定です。`actions.request_ref` はあるのにリクエスト必須項目にないため、再送で `send/kill` が重複実行されます（`docs/agtmux-spec.md:311`, `docs/agtmux-spec.md:545`）。`request_id` 必須化と重複時の同一 `action_id` 返却を先に固定すべきです。  
2. `Critical` セキュリティ要件が矛盾しています。NFR-9 は「unredacted payload を plaintext DB に保存しない」ですが、別節で debug 時の保存を許可しています（`docs/agtmux-spec.md:94`, `docs/agtmux-spec.md:332`）。保存先・暗号化・TTL を統一契約化しないと実装が割れます。  
3. `High` `watch --since` の型が不一致です。CLIは `timestamp`、JSONL/APIは `sequence` 前提です（`docs/agtmux-spec.md:415`, `docs/agtmux-spec.md:466`, `docs/agtmux-spec.md:537`）。互換性テスト以前に契約不整合です。  
4. `High` 「1 pane に active runtime 1件」の規則がDB制約に落ちていません（`docs/agtmux-spec.md:216`, `docs/agtmux-spec.md:238`）。部分ユニークインデックスが必要です。  
5. `High` Phase 1 で `attach` を提供する一方、Exit Criteriaに `attach` の fail-closed 検証がありません（`docs/agtmux-spec.md:583`, `docs/agtmux-spec.md:486`, `docs/agtmux-spec.md:586`）。FR-15の完了判定が抜けています。  
6. `High` `action_snapshot.expires_at` はあるのに、失効時のエラー契約が未定義です（`docs/agtmux-spec.md:306`, `docs/agtmux-spec.md:552`）。`E_SNAPSHOT_EXPIRED` などを先に固定すべきです。  
7. `Medium` pending-bind TTL は「期限切れでdrop」までしかなく、既定値がありません（`docs/agtmux-spec.md:378`）。実装とテストが固定できません。  
8. `Medium` 「partial-result operation」を要求しているのに、レスポンスに `partial_errors` などの必須フィールドがありません（`docs/agtmux-spec.md:92`, `docs/agtmux-spec.md:446`）。  
9. `Medium` `p95 <= 2s` の計測条件（対象環境、target数、pane数、負荷）が未定義です（`docs/agtmux-spec.md:86`, `docs/agtmux-spec.md:589`）。

**欠けている検証項目（追加必須）**
1. 同一イベント集合の順序シャッフル耐性（deterministic convergence）プロパティテスト。  
2. clock skew 境界（±10s）と `effective_event_time` 分岐の境界値テスト。  
3. pending-bind の `bind_no_candidate / bind_ambiguous / bind_ttl_expired` 網羅テスト。  
4. target down -> `unknown/target_unreachable` への収束時間テスト。  
5. `attach` を含む全アクションの stale runtime 拒否テスト。  
6. action retry の冪等性テスト（同一 `request_id` で単一実行）。  
7. `watch` cursor 再開テスト（再起動後・フィルタ変更時の挙動）。  
8. JSON/JSONL スキーマ互換（v1 minor互換）ゴールデンテスト。  
9. retention purge後の参照整合性テスト。  
10. `p95 <= 2s` の負荷試験（条件固定したベンチ）。

**先に固定すべき契約**
1. `watch` cursor 契約。`--since` は `stream_id:sequence` に統一。  
2. Action冪等性契約。`request_id` 必須、重複時は既存結果返却。  
3. Snapshot失効契約。TTL既定値と `E_SNAPSHOT_EXPIRED` を定義。  
4. Partial failure 契約。全 read/watch 応答に `partial_errors[]` を定義。  
5. Security契約。unredacted payload は DB 永続禁止（debug時も）。  
6. Active runtime 制約。DBで `(target_id,pane_id) WHERE ended_at IS NULL` を一意化。

**修正案（貼り付け用diff）**
```diff
--- a/docs/agtmux-spec.md
+++ b/docs/agtmux-spec.md
@@
-- `agtmux watch [--target <name>|--all-targets] [--scope panes|windows|sessions] [--format table|jsonl] [--interval <duration>] [--since <timestamp>] [--once]`
+- `agtmux watch [--target <name>|--all-targets] [--scope panes|windows|sessions] [--format table|jsonl] [--interval <duration>] [--since <cursor>] [--once]`
+  - `<cursor> ::= <stream_id> ":" <sequence>`
+  - `stream_id = sha256(scope + normalized_filters + schema_major)`
@@
-Action request minimum fields:
-- `ref`
+Action request minimum fields:
+- `request_id` (required, UUIDv7)
+- `ref`
@@
 Error code contract (minimum):
@@
 - `E_TARGET_UNREACHABLE`
+- `E_SNAPSHOT_EXPIRED`
+- `E_CURSOR_SCOPE_MISMATCH`
+- `E_IDEMPOTENCY_CONFLICT`
```

```diff
--- a/docs/agtmux-spec.md
+++ b/docs/agtmux-spec.md
@@
-- Full raw payload capture (unredacted) is allowed only in explicit debug mode and MUST expire by retention TTL.
+- Unredacted payload MUST NOT be persisted in SQLite in any mode.
+- Debug mode may keep unredacted payload only in memory or encrypted temp file, with max TTL 24h, and never in `events.raw_payload`.
```

```diff
--- a/docs/agtmux-spec.md
+++ b/docs/agtmux-spec.md
@@
 Exit criteria:
-- manual polling no longer required for Claude/Codex workflows across host and VM targets.
+- `attach` included, all actions implemented in this phase pass fail-closed stale-runtime tests.
 - listing and watch remain usable with partial target failures.
 - visibility latency target is met (`p95 <= 2s` on supported environments).
 - watch jsonl schema contract is validated by compatibility tests.
+  - Benchmark profile is fixed: targets=3, panes=60, update_rate=10 events/s.
```

**コード例**
```sql
-- Runtime uniqueness (DB強制)
CREATE UNIQUE INDEX IF NOT EXISTS ux_runtimes_active
ON runtimes(target_id, pane_id)
WHERE ended_at IS NULL;

-- Action idempotency
CREATE UNIQUE INDEX IF NOT EXISTS ux_actions_request_ref
ON actions(request_ref);
```

```go
func TestReplayDeterminism(t *testing.T) {
    events := loadFixtureEvents("out_of_order.json")
    want := runEngine(events).StateHash()
    for i := 0; i < 500; i++ {
        got := runEngine(shuffle(events, int64(i))).StateHash()
        if got != want { t.Fatalf("non-deterministic convergence: run=%d", i) }
    }
}

func TestSendIdempotency(t *testing.T) {
    req := ActionRequest{RequestID: "018f...uuidv7", Ref: "runtime:abc...", Type: "send"}
    a1 := postAction(req)
    a2 := postAction(req) // retry
    if a1.ActionID != a2.ActionID { t.Fatal("idempotency broken") }
}
```

