**重大度順の指摘**

1. **Critical: `implementation-plan` が要求するテストID (`TC-040`〜`TC-043`) が `test-catalog` に存在しない**
根拠: `docs/implementation-plan.md:64`, `docs/implementation-plan.md:89`, `docs/test-catalog.md:22`, `docs/test-catalog.md:62`  
影響: フェーズゲート条件が満たしようがなく、計画と検証が分断されています。

2. **Critical: FR/NFR の未トレース要件がある（Spec→Task→Test が切れている）**
根拠: `docs/agtmux-spec.md:73`, `docs/agtmux-spec.md:75`, `docs/agtmux-spec.md:77`, `docs/agtmux-spec.md:89`, `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:91`, `docs/tasks.md:10`, `docs/test-catalog.md:22`  
未接続: `FR-7`, `FR-9`, `FR-11`, `NFR-4`, `NFR-5`, `NFR-6`。

3. **High: Attach の実装順序が Spec/Plan と Task/Gate で矛盾**
根拠: `docs/agtmux-spec.md:505`, `docs/agtmux-spec.md:510`, `docs/agtmux-spec.md:564`, `docs/implementation-plan.md:75`, `docs/implementation-plan.md:78`, `docs/implementation-plan.md:154`, `docs/tasks.md:26`, `docs/tasks.md:29`, `docs/test-catalog.md:69`, `docs/test-catalog.md:70`  
影響: Attach が必要とする idempotency 基盤より先に実装される依存逆転が起きています。

4. **High: Phase gate の束ね方が Plan と Test Catalog で不一致**
根拠: `docs/implementation-plan.md:63`, `docs/implementation-plan.md:89`, `docs/test-catalog.md:68`, `docs/test-catalog.md:69`  
影響: Plan が要求する `TC-011/012/013`（Phase 0）や `TC-042/043`（Phase 1）がゲート束に含まれていません。

5. **Medium: FR/NFR列に `Goal` が残り、トレーサビリティ規約を崩している**
根拠: `docs/tasks.md:40`, `docs/tasks.md:41`, `docs/test-catalog.md:61`  
影響: FR/NFRベースの追跡ができず、監査時に曖昧になります。

6. **Low: Task と Test の requirementラベルが局所不整合**
根拠: `docs/tasks.md:23`, `docs/test-catalog.md:41`  
内容: `TASK-012` は `FR-4, FR-6` なのに、紐づく `TC-018` は `FR-16` 起点。

---

**反映すべき最小修正（差分例）**

`docs/tasks.md`
```diff
@@
-| TASK-004 | 0 | P0 | Implement event ordering and dedupe comparator | FR-14, NFR-8 | TASK-001 | Deterministic output for shuffled identical streams | TC-006, TC-007 | Todo |
+| TASK-004 | 0 | P0 | Implement event ordering and dedupe comparator | FR-14, NFR-4, NFR-8 | TASK-001 | Deterministic output for shuffled identical streams | TC-006, TC-007 | Todo |
@@
-| TASK-012 | 1 | P0 | Implement API v1 read endpoints | FR-4, FR-6 | TASK-004, TASK-006 | panes/windows/sessions JSON contract stable | TC-017, TC-018 | Todo |
+| TASK-012 | 1 | P0 | Implement API v1 read endpoints | FR-4, FR-6, FR-16 | TASK-004, TASK-006 | panes/windows/sessions JSON contract stable | TC-017, TC-018 | Todo |
@@
-| TASK-014 | 1 | P0 | Implement CLI list/watch mapping to API v1 | FR-4, FR-6 | TASK-012, TASK-013 | CLI output and JSON align with API schema | TC-021 | Todo |
+| TASK-014 | 1 | P0 | Implement CLI list/watch mapping to API v1 | FR-4, FR-6, FR-7, FR-9 | TASK-012, TASK-013 | CLI output and JSON align with API schema incl. grouping/aggregation semantics | TC-021, TC-042, TC-043 | Todo |
@@
-| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
+| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012, TASK-018 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
@@
-| TASK-018 | 1.5 | P0 | Implement `actions` write path with idempotency | FR-15 | TASK-015 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
+| TASK-018 | 1 | P0 | Implement shared `actions` write path with idempotency (attach-ready) | FR-15 | TASK-012 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
@@
-| TASK-027 | 2.5 | P1 | Add Copilot CLI adapter | FR-12 | TASK-024 | adapter passes shared contract tests | TC-036 | Todo |
+| TASK-027 | 2.5 | P1 | Add Copilot CLI adapter | FR-11, FR-12, NFR-5, NFR-6 | TASK-024 | adapter passes shared contract tests without core changes | TC-036 | Todo |
-| TASK-028 | 2.5 | P1 | Add Cursor CLI adapter | FR-12 | TASK-024 | adapter passes shared contract tests | TC-037 | Todo |
+| TASK-028 | 2.5 | P1 | Add Cursor CLI adapter | FR-11, FR-12, NFR-5, NFR-6 | TASK-024 | adapter passes shared contract tests without core changes | TC-037 | Todo |
@@
-| TASK-029 | 3 | P1 | Build macOS app read views using API v1 | Goal | TASK-012, TASK-013 | app can render global/session/window/pane views | TC-038 | Todo |
+| TASK-029 | 3 | P1 | Build macOS app read views using API v1 | FR-4, FR-6, FR-9 | TASK-012, TASK-013 | app can render global/session/window/pane views | TC-038 | Todo |
-| TASK-030 | 3 | P1 | Build macOS app actions with same safety checks | Goal, FR-15 | TASK-018, TASK-019, TASK-021 | app actions preserve fail-closed semantics | TC-039 | Todo |
+| TASK-030 | 3 | P1 | Build macOS app actions with same safety checks | FR-5, FR-15 | TASK-018, TASK-019, TASK-021 | app actions preserve fail-closed semantics | TC-039 | Todo |
+| TASK-031 | 0 | P1 | Implement TargetExecutor local/ssh parity | FR-9, FR-10 | TASK-001 | local/ssh execution parity and error mapping are stable | TC-040 | Todo |
+| TASK-032 | 0 | P1 | Implement tmux topology observer per target | FR-3, FR-9 | TASK-031 | topology converges without ghost panes on churn | TC-041 | Todo |
```

`docs/test-catalog.md`
```diff
@@
-| TC-006 | Property | Ordering determinism | FR-14, NFR-8 | shuffle same event set repeatedly | identical final state hash | CI |
+| TC-006 | Property | Ordering determinism | FR-14, NFR-4, NFR-8 | shuffle same event set repeatedly | identical final state hash | CI |
-| TC-007 | Unit | Dedupe behavior | FR-14 | duplicate event submission | single logical apply | CI |
+| TC-007 | Unit | Dedupe behavior | FR-14, NFR-4 | duplicate event submission | single logical apply | CI |
@@
-| TC-036 | Integration | Copilot adapter contract suite | FR-12 | adapter integration fixtures | core engine unchanged | CI |
+| TC-036 | Integration | Copilot adapter contract suite | FR-11, FR-12, NFR-5, NFR-6 | adapter integration fixtures | core engine unchanged + minor-version contract compatibility maintained | CI |
-| TC-037 | Integration | Cursor adapter contract suite | FR-12 | adapter integration fixtures | core engine unchanged | CI |
+| TC-037 | Integration | Cursor adapter contract suite | FR-11, FR-12, NFR-5, NFR-6 | adapter integration fixtures | core engine unchanged + minor-version contract compatibility maintained | CI |
-| TC-038 | E2E | macOS app read parity | Goal | app screens vs API v1 data | parity validated | Manual+CI |
+| TC-038 | E2E | macOS app read parity | FR-4, FR-6, FR-9 | app screens vs API v1 data | parity validated | Manual+CI |
+| TC-040 | E2E | TargetExecutor local/ssh parity | FR-9, FR-10 | same tmux read/write ops on local and ssh targets | equivalent behavior and stable error mapping | CI |
+| TC-041 | Integration | Topology observer convergence | FR-3, FR-9 | pane/window churn across multiple targets | canonical topology converges with no stale rows | CI |
+| TC-042 | E2E | Multi-target aggregated listing semantics | FR-9 | host/vm mixed state with one degraded target | aggregated view keeps target identity and valid partial results | CI |
+| TC-043 | Contract | Grouping/count summary correctness | FR-7 | `target-session` grouping with mixed pane states | per-group/per-state counts are deterministic and correct | CI |
@@
-| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010 |
+| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010, TC-011, TC-012, TC-013, TC-040, TC-041 |
-| Phase 1 close | Phase 0 bundle + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024 |
+| Phase 1 close | Phase 0 bundle + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024, TC-025, TC-026, TC-042, TC-043 |
-| Phase 1.5 close | Phase 1 bundle + TC-025, TC-026, TC-027, TC-028, TC-029, TC-030, TC-031, TC-032 |
+| Phase 1.5 close | Phase 1 bundle + TC-027, TC-028, TC-029, TC-030, TC-031, TC-032 |
```

`docs/implementation-plan.md`（明確化の最小追記）
```diff
@@
-- Attach stale-runtime rejection tests pass.
+- Attach stale-runtime rejection tests pass (`TC-022`).
+- Attach idempotency replay/conflict tests pass (`TC-025`, `TC-026`).
```

上記を反映すれば、`FR/NFR → Task → Test` の断絶と Plan/Task/Test の矛盾は最小変更で解消できます。
