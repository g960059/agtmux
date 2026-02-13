最終判定は `Stop` です。`docs/test-catalog.md` は品質ゲートとして重要な抜けがあり、このままだとフェーズ通過判定が甘くなります。

1. Severity: Critical | File: `docs/test-catalog.md:35`, `docs/test-catalog.md:36`, `docs/test-catalog.md:37`, `docs/test-catalog.md:68`  
理由: セキュリティ/性能系の `TC-011/012/013` が定義されているのに、どのフェーズゲートにも含まれていません。NFR-9 と NFR-1/NFR-3 が未検証でもフェーズを閉じられます。  
修正案: `Phase 0 close` か `Phase 1 close` に `TC-011` と `TC-013` を必須化し、`TC-012` を「Phase close 前に直近成功必須」の扱いへ変更。  
Confidence: High

2. Severity: Critical | File: `docs/test-catalog.md:69`, `docs/test-catalog.md:75`, `docs/implementation-plan.md:80`, `docs/implementation-plan.md:81`  
理由: Phase 1 の exit criteria（可視化遅延 p95 <= 2s、watch schema 互換）に対応する明示テストがフェーズゲートにありません。計画とゲートが不整合です。  
修正案: `TC-044`（可視化遅延SLO）と `TC-045`（watch JSONL 互換）を追加し、Phase 1 close に必須化。  
Confidence: High

3. Severity: High | File: `docs/test-catalog.md:22`, `docs/test-catalog.md:63`, `docs/agtmux-spec.md:73`, `docs/agtmux-spec.md:75`, `docs/agtmux-spec.md:77`, `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:91`  
理由: `FR-7/FR-9/FR-11/NFR-5/NFR-6` の直接トレースがありません（要件網羅不足）。  
修正案: grouping/count、multi-target衝突回避、adapter registry 非改修拡張、adapter contract後方互換を独立テスト化。  
Confidence: High

4. Severity: High | File: `docs/test-catalog.md:29`, `docs/test-catalog.md:61`, `docs/test-catalog.md:63`, `docs/test-catalog.md:75`, `docs/test-catalog.md:90`  
理由: 再現性ルールが欠落しています（property seed固定、ベンチ実行条件、Manual+CI 手順/証跡）。フレークでゲート信頼性が落ちます。  
修正案: 再現性セクションを追加し、seed・fixture hash・clock固定・実行回数・失敗時再試行条件を規定。  
Confidence: High

5. Severity: High | File: `docs/test-catalog.md:57`  
理由: 信頼性試験が `TC-034` の広い記述に寄りすぎで、クラッシュ再起動・SQLiteロック・watch再開整合の検証がありません。  
修正案: restart/replay/cursor continuity/DB transient failure を分離して追加。  
Confidence: Medium

6. Severity: Medium | File: `docs/test-catalog.md:41`, `docs/test-catalog.md:55`, `docs/agtmux-spec.md:577`  
理由: エラーコード契約（最低12種）が包括的に網羅されていません。`TC-018/032` だけだと漏れます。  
修正案: 全コードを table-driven で網羅する `TC-049` を追加。  
Confidence: Medium

**追加テスト提案（新規ID）**
- `TC-040` E2E: `target-session` 既定グルーピングと state count 集計の正当性（FR-7, FR-9）
- `TC-041` E2E: 同名 session の cross-target 衝突回避（FR-9）
- `TC-042` Integration: adapter registry 経由追加で core 変更不要を検証（FR-11, NFR-5）
- `TC-043` Contract: adapter contract minor version 後方互換（NFR-6）
- `TC-044` Performance: benchmark profile 条件で visible lag p95 <= 2s（NFR-1）
- `TC-045` Contract: watch JSONL schema snapshot compatibility（Phase 1 gate）
- `TC-046` Resilience: daemon restart 後の replay determinism（重複適用なし）
- `TC-047` E2E: watch cursor の restart 跨ぎ連続性（欠落/重複なし）
- `TC-048` Integration: skew_budget 超過時の `ingested_at` フォールバック順序
- `TC-049` Contract: 全エラーコードの API/CLI 一貫性
- `TC-050` Security: debug mode でも SQLite に生ペイロード不保存（NFR-9）
- `TC-051` Resilience: SQLite busy/lock 時の retry/backoff と整合性
- `TC-052` Integration: 同一 `request_ref` 競合同時実行の厳密冪等

**フェーズゲート修正案（差分イメージ）**
```diff
- Phase 0 close: TC-001..TC-010
+ Phase 0 close: TC-001..TC-010, TC-011, TC-013, TC-050

- Phase 1 close: Phase 0 + TC-014..TC-024
+ Phase 1 close: Phase 0 + TC-014..TC-024, TC-040, TC-041, TC-044, TC-045, TC-049

- Phase 1.5 close: Phase 1 + TC-025..TC-032
+ Phase 1.5 close: Phase 1 + TC-025..TC-032, TC-052

- Phase 2 close: Phase 1.5 + TC-033, TC-034, TC-035
+ Phase 2 close: Phase 1.5 + TC-033, TC-034, TC-035, TC-043, TC-046, TC-047, TC-048, TC-051

- Phase 2.5 close: Phase 2 + TC-036, TC-037
+ Phase 2.5 close: Phase 2 + TC-036, TC-037, TC-042
```

**コード例（再現性つき property test）**
```go
func TestOrderingDeterminism(t *testing.T) {
	seed := int64(20260213)
	rng := rand.New(rand.NewSource(seed))
	base := loadFixture(t, "fixtures/events/mixed_stream_v1.json")
	want := ""

	for i := 0; i < 200; i++ {
		ev := slices.Clone(base)
		rng.Shuffle(len(ev), func(a, b int) { ev[a], ev[b] = ev[b], ev[a] })
		state := Replay(ev, WithFixedClock(time.Unix(1760000000, 0)))
		got := state.Hash()

		if i == 0 {
			want = got
			continue
		}
		if got != want {
			t.Fatalf("non-deterministic convergence seed=%d iter=%d got=%s want=%s", seed, i, got, want)
		}
	}
}
```

**コード例（エラーコード網羅テスト）**
```go
func TestErrorCodeContract(t *testing.T) {
	cases := []struct {
		name string
		req  Request
		code string
	}{
		{"invalid ref", badRefReq(), "E_REF_INVALID"},
		{"invalid encoding", badEncodingReq(), "E_REF_INVALID_ENCODING"},
		{"not found", notFoundReq(), "E_REF_NOT_FOUND"},
		{"ambiguous", ambiguousReq(), "E_REF_AMBIGUOUS"},
		{"runtime stale", staleRuntimeReq(), "E_RUNTIME_STALE"},
		{"precondition", preconditionReq(), "E_PRECONDITION_FAILED"},
		{"snapshot expired", expiredSnapshotReq(), "E_SNAPSHOT_EXPIRED"},
		{"idempotency conflict", conflictReq(), "E_IDEMPOTENCY_CONFLICT"},
		{"cursor invalid", badCursorReq(), "E_CURSOR_INVALID"},
		{"cursor expired", expiredCursorReq(), "E_CURSOR_EXPIRED"},
		{"pid unavailable", missingPidReq(), "E_PID_UNAVAILABLE"},
		{"target unreachable", downTargetReq(), "E_TARGET_UNREACHABLE"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			resp := callAPI(tc.req)
			if resp.Error.Code != tc.code {
				t.Fatalf("got=%s want=%s", resp.Error.Code, tc.code)
			}
		})
	}
}
```
