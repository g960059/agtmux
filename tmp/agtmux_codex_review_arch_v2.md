判定は **Stop（設計修正を先に入れるべき）** です。`runtime_id/pane_epoch` と制御コマンド安全性の定義不足が、誤操作と状態破綻に直結します。

**Critical**
1. 制御コマンドの fail-closed が実質任意で、誤対象操作を防げない  
根拠: `docs/agtmux-spec.md:81`（FR-15 は「support」止まり）、`docs/agtmux-spec.md:287`, `docs/agtmux-spec.md:289`（`--if-runtime` 任意）、`docs/agtmux-spec.md:316`, `docs/agtmux-spec.md:326`（mismatch 時のみ fail-closed）。  
具体改善案: `send/kill/attach` は `action_token` 必須（`runtime_id + state_version + freshness + nonce`）。`--force-stale` を明示しない限り実行不可にする。  
修正例:
```go
type ActionToken struct {
  TargetID      string
  PaneID        string
  RuntimeID     string
  StateVersion  int64
  ExpiresAtUnix int64
  Nonce         string
}

func (s *Service) Kill(ctx context.Context, req KillReq) error {
  if req.Token == "" && !req.ForceStale { return ErrPreconditionRequired }
  st := s.store.GetStateForUpdate(req.TargetID, req.PaneID)
  tok := parseAndVerify(req.Token)
  if st.RuntimeID != tok.RuntimeID || st.StateVersion != tok.StateVersion || nowUnix() > tok.ExpiresAtUnix {
    return ErrPreconditionFailed
  }
  return s.exec.Signal(ctx, req.TargetID, req.PaneID, req.Signal)
}
```

2. `runtime_id` 必須なのに、初期イベントの runtime binding 規約が未定義  
根拠: `docs/agtmux-spec.md:250`（event envelope で `runtime_id` 必須）と `docs/agtmux-spec.md:257`-`267`（hook/notify/parser 由来イベントは通常 `runtime_id` を直接持てない）。  
具体改善案: `pending_bind -> bound -> applied` の 2段階取り込みを仕様化し、`runtime_ref(target,pane,pid,start_hint)` で解決する。解決不能イベントは inbox で保持し TTL 後に `dropped_unbound`。  
修正例:
```sql
CREATE TABLE event_inbox (
  inbox_id TEXT PRIMARY KEY,
  target_id TEXT NOT NULL,
  pane_id TEXT NOT NULL,
  runtime_id TEXT NULL,
  pid INTEGER NULL,
  source TEXT NOT NULL,
  dedupe_key TEXT NOT NULL,
  event_time INTEGER NOT NULL,
  payload BLOB NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending_bind','bound','applied','dropped_unbound'))
);
```

**High**
1. `agtmuxd` 境界が論理説明のみで、障害分離・責務分割が不足  
根拠: `docs/agtmux-spec.md:104`-`110`, `docs/agtmux-spec.md:347`。  
具体改善案: `collector(各target, 別プロセス)` と `agtmuxd(API+state)` をプロセス境界で分離。collector は append-only queue に書き、daemon が適用。片側障害で全体停止しない構成を明記。

2. `TargetExecutor` が抽象名のみで、timeout/retry/error-classification が未規定  
根拠: `docs/agtmux-spec.md:101`-`103`。  
具体改善案: 実行契約を定義（`ReadOnly/Mutating`, `Timeout`, `IdempotencyKey`, `ErrCode`）。ssh/local の差を executor 内に閉じ込め、上位は `ErrCode` のみ参照。  
修正例:
```go
type ErrCode string
const (
  ErrTimeout ErrCode = "TIMEOUT"
  ErrUnreachable ErrCode = "UNREACHABLE"
  ErrAuth ErrCode = "AUTH_FAILED"
  ErrNonZero ErrCode = "NON_ZERO_EXIT"
)
```

3. FR-14 の「deterministic ordering」がアルゴリズム未定義  
根拠: `docs/agtmux-spec.md:80`, `docs/agtmux-spec.md:154`, `docs/agtmux-spec.md:159`, `docs/agtmux-spec.md:253`。  
具体改善案: 比較キーと tie-break を固定化（例: `source_seq` > `event_time` > `ingested_at` > `event_id(UUIDv7)`）。`last_applied_key` を runtime 単位で保存し、過去キーは drop。

4. `pane_epoch` の増分条件が不明で、pane再利用/再起動時に衝突し得る  
根拠: `docs/agtmux-spec.md:79`, `docs/agtmux-spec.md:209`-`214`。  
具体改善案: `tmux_boot_id` を導入し、`(target_id, tmux_boot_id, pane_id)` を pane instance として定義。instance 変化時に epoch を必ずインクリメントする規則を明文化。

**Medium**
1. Adapter versioning が文字列返却のみで互換性保証に弱い  
根拠: `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:168`。  
具体改善案: `engine_semver_range` と `adapter_semver` を registry に保持し、起動時 handshake で reject/allow 判定。

2. Copilot/Cursor 拡張性が capability の粒度不足で将来 core 変更を誘発  
根拠: `docs/agtmux-spec.md:176`-`180`, `docs/agtmux-spec.md:264`-`267`, `docs/agtmux-spec.md:387`-`393`。  
具体改善案: capability を状態有無だけでなく、`signal_quality`, `latency_class`, `requires_wrapper`, `locale_sensitive_parser` まで拡張。

3. actions と events の監査相関が schema 上で未接続  
根拠: `docs/agtmux-spec.md:215`-`226`, `docs/agtmux-spec.md:370`-`372`。  
具体改善案: `actions` テーブル追加、`events.action_id` 参照で因果追跡を可能化。

4. reconciler 負荷制御の定量指標がなくスケール時に過負荷化  
根拠: `docs/agtmux-spec.md:270`-`272`。  
具体改善案: target ごとの scan budget と jitter、最大同時実行数、優先度（waiting系優先）を仕様化。

**Low**
1. `attach` の freshness が `should` で安全基準が弱い  
根拠: `docs/agtmux-spec.md:319`。  
具体改善案: `attach` も `must validate freshness` に統一し、失敗時は `--force-stale` のみ許可。

2. `--all-targets` 既定のまま大規模環境で UX/性能劣化リスク  
根拠: `docs/agtmux-spec.md:430`-`433`。  
具体改善案: target 数しきい値で既定を切替（例: >10 は current-target 既定）または server-side pagination を先行導入。

**設計破綻リスクと回避策**
1. 誤 Kill/Send（別 runtime へ誤着弾）  
回避策: `action_token` 必須化 + compare-and-swap 実行。

2. 状態フラップ/非決定収束（再起動ごとに結果が変わる）  
回避策: ordering comparator と `last_applied_key` 永続化。

3. collector 障害で全体停止（境界曖昧な単一障害点）  
回避策: collector/daemon 分離 + queue 経由 + partial-result 維持。

4. 新 adapter 導入時に core 侵食（Copilot/Cursorで例外分岐増殖）  
回避策: capability 拡張と contract test suite を先に固定。

**反映推奨 TOP5**
1. `action_token` 必須化（`send/kill/attach` の安全モデルを先に固定）。  
2. `runtime binding` の 2段階 ingestion（pending_bind inbox）を追加。  
3. `pane_epoch` 生成規則（`tmux_boot_id` 含む）を明文化。  
4. `agtmuxd` 境界をプロセス/責務/障害ドメインで定義。  
5. FR-14 の ordering/dedupe アルゴリズムを比較キーまで仕様化。