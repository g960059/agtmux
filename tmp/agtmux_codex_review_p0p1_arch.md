最終判定は `Stop` です。`runtime_id/pane_epoch` と `pending_bind` に、誤バインドを起こしうる Blocking 問題があります。

1. **Critical** `docs/agtmux-spec.md:216` `docs/agtmux-spec.md:248`  
`「1 pane に active runtime は1つ」` が仕様要求なのに、スキーマで強制されていません。並行更新で `ended_at IS NULL` が複数行になりえます。`E_RUNTIME_STALE` 判定の前提が崩れます。  
改善案: 部分ユニークインデックス + ランタイム切替を単一トランザクションで規定してください。  
```sql
CREATE UNIQUE INDEX uq_runtimes_one_active
ON runtimes(target_id, pane_id)
WHERE ended_at IS NULL;
```

2. **Critical** `docs/agtmux-spec.md:370` `docs/agtmux-spec.md:371` `docs/agtmux-spec.md:374`  
`pending_bind` が `target_id + pane_id` ベースで解決可能になっており、遅延イベントが次 epoch に誤バインドされる余地があります。  
改善案: 強いヒントなしは bind しない fail-closed に変更。`tmux_server_boot_id` を envelope/inbox に追加し、`event_time >= runtime.started_at - skew` も必須化。  
```diff
- Resolver MUST bind only when there is exactly one active runtime candidate ...
+ Resolver MUST bind only when:
+ (a) exactly one candidate remains, and
+ (b) at least one strong hint exists (runtime_id|pid|start_hint|tmux_server_boot_id), and
+ (c) event_time is within runtime lifetime/skew window.
+ Otherwise status MUST become dropped_unbound (reason_code=bind_insufficient_hints|...).
```

3. **High** `docs/agtmux-spec.md:486` `docs/agtmux-spec.md:487` `docs/agtmux-spec.md:550`  
fail-closed の実チェック条件が曖昧です。`--force-stale` が `send/kill` にも効く設計だと、誤ペイン操作リスクが残ります。  
改善案: サーバで必須比較項目を規範化 (`runtime_id`,`state_version`,`snapshot TTL`,`target health`)。`send/kill` では `force_stale` 無効化か、さらに厳格条件を追加。  
```go
if cur.RuntimeID != snap.RuntimeID { return E_RUNTIME_STALE }
if cur.StateVersion != snap.StateVersion { return E_PRECONDITION_FAILED }
if now.After(snap.ExpiresAt) { return E_PRECONDITION_FAILED }
if target.Health == "down" { return E_TARGET_UNREACHABLE }
```

4. **High** `docs/agtmux-spec.md:261` `docs/agtmux-spec.md:277`  
dedupe の一意性スコープが `events` と `event_inbox` で不整合です。runtime境界で「重複誤判定」または「二重適用」が起きえます。  
改善案: `bind_scope_id`（例: `target_id+tmux_server_boot_id+pane_id+pane_epoch`）を導入し、pending/bound 両方で同一スコープ dedupe を統一してください。

5. **Medium** `docs/agtmux-spec.md:213` `docs/agtmux-spec.md:215`  
`runtime_id` を「再現可能ハッシュ」にすると、`started_at_ns` 精度差や観測差分で runtime 分裂しやすいです。  
改善案: `runtime_id` は daemon 発番（UUIDv7/ULID）にし、再現用は `runtime_fingerprint` 別列に分離。

6. **Medium** `docs/agtmux-spec.md:583` `docs/agtmux-spec.md:594`  
Phase整合性が弱いです。Phase1 で `attach` を出す一方、fail-closed 実装は Phase1.5 の記述が中心です。  
改善案: Phase1 exit criteria に `attach` の snapshot 検証通過を明記し、FR-15 と揃えてください。

**不足テスト（追加推奨）**
1. 同一 `target_id/pane_id` で同時 runtime 回転時に active 1件を維持する競合テスト。  
2. 遅延 old-event が新 epoch に bind されないことのプロパティテスト。  
3. `force_stale` あり/なしで `attach` と `kill` の許可マトリクス検証。  
4. pending→bound 再試行で状態遷移が exactly-once になる冪等テスト。  
5. clock skew 境界（±10s）で ordering が決定的に収束するテスト。
