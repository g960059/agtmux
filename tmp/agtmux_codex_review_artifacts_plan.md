**判定**
Stop（依存順序とゲート定義に契約違反レベルの欠落があります）

**指摘（重大度順）**
1. **Critical** `attach` のフェーズ配置が fail-closed 契約と衝突しています。  
対象: `docs/agtmux-spec.md:506`, `docs/agtmux-spec.md:510`, `docs/implementation-plan.md:71`, `docs/implementation-plan.md:91`, `docs/tasks.md:26`, `docs/tasks.md:29`  
理由: Spec は「全 action で `actions` + `action_snapshot` + idempotency」を要求していますが、`attach` は Phase 1、idempotency/write path は Phase 1.5 になっており順序が逆です。  
修正案: `TASK-018` を Phase 1 に前倒しし、`TASK-015` の依存に `TASK-018` を追加。

2. **Critical** Adapter expansion の依存が実装計画のルール違反です。  
対象: `docs/implementation-plan.md:144`, `docs/tasks.md:38`, `docs/tasks.md:39`  
理由: Plan は「Gemini reliability baseline 後に拡張」と定義しているのに、`TASK-027/028` は `TASK-024` のみ依存です。  
修正案: `TASK-027/028` の依存を `TASK-025` と `TASK-026`（必要なら registry task）まで引き上げる。

3. **High** Phase 0 gate から Spec 必須のセキュリティ/性能基盤テストが漏れています。  
対象: `docs/tasks.md:18`, `docs/tasks.md:19`, `docs/test-catalog.md:68`, `docs/agtmux-spec.md:335`, `docs/agtmux-spec.md:342`  
理由: `TC-011/012/013` が Phase 0 close 条件に含まれず、平文 payload 抑止や index 基盤未達で進行可能です。  
修正案: Phase 0 close bundle に `TC-011, TC-012, TC-013` を追加し、`implementation-plan` の Exit criteria に明記。

4. **High** Spec Phase 0 の必須成果物（`TargetExecutor`/tmux observer/daemon boundary）が計画・タスク・テストに未展開です。  
対象: `docs/agtmux-spec.md:603`, `docs/agtmux-spec.md:605`, `docs/implementation-plan.md:48`, `docs/tasks.md:10`, `docs/test-catalog.md:22`  
理由: 後続フェーズの前提コンポーネントがタスク化されていません。  
修正案: Phase 0 に専用タスクとテストID（例: `TC-040`,`TC-041`）を追加。

5. **High** FR/NFR のフェーズ整合欠落（FR-7, FR-9, FR-11, NFR-5, NFR-6）。  
対象: `docs/agtmux-spec.md:73`, `docs/agtmux-spec.md:75`, `docs/agtmux-spec.md:77`, `docs/agtmux-spec.md:90`, `docs/agtmux-spec.md:91`, `docs/tasks.md:10`, `docs/test-catalog.md:22`  
理由: 現在の backlog/matrix ではこれらが明示的にトレースされず、ゲート通過しても未実装のままになる余地があります。  
修正案: grouping/aggregated view/adapter registry/contract version compatibility を個別タスク+個別TCで追加。

6. **Medium** 即時スプリント候補が依存関係を満たしていません。  
対象: `docs/tasks.md:50`, `docs/tasks.md:23`, `docs/tasks.md:17`  
理由: `TASK-012` は `TASK-006` 依存ですが、スプリント候補に `TASK-006` が含まれていません。  
修正案: スプリント候補を `001-006` 優先に再編し、`012/013` は `006` 完了後に着手。

7. **Medium** Phase 2 の仕様項目「richer filters/sorting」が計画とテストに未反映です。  
対象: `docs/agtmux-spec.md:645`, `docs/implementation-plan.md:112`, `docs/tasks.md:35`  
理由: JSON schema hardening はあるが、filters/sorting の実装・検証が抜けています。  
修正案: Phase 2 に filters/sorting タスクと contract test を追加。

8. **Medium** Gate 定義が文書間で「説明的」で、厳密なブロッキング条件として結合されていません。  
対象: `docs/implementation-plan.md:31`, `docs/test-catalog.md:66`  
理由: Plan 側が TC-ID 非明示のため、運用時に gate 解釈がぶれます。  
修正案: `implementation-plan` に「Phase close = Required TC bundle all green」を明示した正規表を追加。

---

**具体的な修正案（差分例）**

`docs/implementation-plan.md`
```diff
@@ Phase 0: Core Runtime / In scope
+- Implement `TargetExecutor` and daemon boundary (`agtmuxd` service/API boundary).
+- Implement tmux topology observer per target.

@@ Phase 0: Core Runtime / Exit criteria
+- Security/performance baseline enforced: `TC-011`, `TC-012`, `TC-013` pass.

@@ Phase 1: Visibility MVP / In scope
-- Attach with fail-closed snapshot validation.
+- Attach with fail-closed snapshot validation through shared `actions` + `action_snapshots`.
+- Expose minimal write endpoint `POST /v1/actions/attach` in Phase 1.

@@ Phase 1.5: Control MVP / In scope
-- API v1 write endpoints.
-- Idempotent request handling (`request_ref`).
+- API v1 write endpoints for `send`, `view-output`, `kill` (attach path already in Phase 1).
+- Shared idempotent request handling (`request_ref`) for all action types.

@@ Phase 2: Gemini + Reliability Hardening / In scope
+- Richer filters/sorting for list/watch in addition to schema hardening.

@@ 6. Dependency and Sequence Rules
-- No adapter expansion before Gemini reliability baseline is reached.
+- No adapter expansion before Phase 2 gate bundle (`TC-033`,`TC-034`,`TC-035`) is green.
+- Attach shipment requires action snapshot + idempotency core (`TC-022`,`TC-025`,`TC-026`) green.
```

`docs/tasks.md`
```diff
@@
-| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012 | Attach rejects stale runtime/snapshot | TC-022 | Todo |
+| TASK-015 | 1 | P0 | Implement attach action with snapshot validation | FR-5, FR-15 | TASK-003, TASK-012, TASK-018 | Attach rejects stale runtime/snapshot | TC-022 | Todo |

-| TASK-018 | 1.5 | P0 | Implement `actions` write path with idempotency | FR-15 | TASK-015 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |
+| TASK-018 | 1 | P0 | Implement shared `actions` write path with idempotency (attach含む) | FR-15, NFR-4 | TASK-012 | same request_ref returns same action_id/result | TC-025, TC-026 | Todo |

-| TASK-027 | 2.5 | P1 | Add Copilot CLI adapter | FR-12 | TASK-024 | adapter passes shared contract tests | TC-036 | Todo |
-| TASK-028 | 2.5 | P1 | Add Cursor CLI adapter | FR-12 | TASK-024 | adapter passes shared contract tests | TC-037 | Todo |
+| TASK-027 | 2.5 | P1 | Add Copilot CLI adapter | FR-12 | TASK-025, TASK-026, TASK-035 | adapter passes shared contract tests | TC-036 | Todo |
+| TASK-028 | 2.5 | P1 | Add Cursor CLI adapter | FR-12 | TASK-025, TASK-026, TASK-035 | adapter passes shared contract tests | TC-037 | Todo |

+| TASK-031 | 0 | P0 | Implement `TargetExecutor` and agtmuxd boundary | FR-9 | TASK-001 | all target read/write paths go through executor abstraction | TC-040 | Todo |
+| TASK-032 | 0 | P0 | Implement tmux topology observer per target | FR-3, FR-9 | TASK-031 | topology snapshots converge across targets | TC-041 | Todo |
+| TASK-033 | 1 | P1 | Implement grouping and summary rollups | FR-7 | TASK-012 | session/window summaries match spec 7.6 | TC-042 | Todo |
+| TASK-034 | 1 | P1 | Enforce aggregated multi-target response semantics | FR-9, NFR-7 | TASK-009, TASK-012, TASK-016 | requested/responded/target_errors consistency | TC-043 | Todo |
+| TASK-035 | 2 | P0 | Implement adapter registry capability-driven dispatch | FR-11, NFR-5 | TASK-024 | add adapter without core engine changes | TC-044 | Todo |
+| TASK-036 | 2 | P1 | Add adapter contract version compatibility checks | NFR-6 | TASK-035 | backward-compatible minor version changes validated | TC-045 | Todo |
+| TASK-037 | 2 | P1 | Add richer filters/sorting for list/watch | FR-4 | TASK-012, TASK-013 | filter/sort contract stable | TC-046 | Todo |
```

`docs/test-catalog.md`
```diff
@@ Contract Test Matrix
+| TC-040 | Integration | TargetExecutor/daemon boundary | FR-9 | mixed local/ssh read-write flows | executor abstraction consistently used | CI |
+| TC-041 | E2E | Multi-target topology observer | FR-3, FR-9 | target reconnect and pane churn | topology converges without stale bleed | CI |
+| TC-042 | Contract | Grouping and summary correctness | FR-7 | panes/windows/sessions rollups | counts/state precedence are correct | CI |
+| TC-043 | Contract | Aggregated multi-target semantics | FR-9, NFR-7 | partial target failure in aggregated read | requested/responded/target_errors are consistent | CI |
+| TC-044 | Integration | Adapter registry extensibility | FR-11, NFR-5 | add mock adapter plugin | no core state engine changes required | CI |
+| TC-045 | Contract | Adapter contract version compatibility | NFR-6 | adapter minor version bump | backward-compatible behavior preserved | CI |
+| TC-046 | Contract | Filter/sort schema stability | FR-4 | list/watch with new filters and sorting | stable output schema + deterministic order | CI |

@@ 3. Phase Gate Bundles
-| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010 |
+| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010, TC-011, TC-012, TC-013, TC-040, TC-041 |
-| Phase 1 close | Phase 0 bundle + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024 |
+| Phase 1 close | Phase 0 bundle + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024, TC-042, TC-043 |
-| Phase 2 close | Phase 1.5 bundle + TC-033, TC-034, TC-035 |
+| Phase 2 close | Phase 1.5 bundle + TC-033, TC-034, TC-035, TC-046 |
-| Phase 2.5 close | Phase 2 bundle + TC-036, TC-037 |
+| Phase 2.5 close | Phase 2 bundle + TC-036, TC-037, TC-044, TC-045 |
```

**最小の依存順序（再定義案）**
```txt
TASK-001 -> TASK-031 -> TASK-032 -> TASK-004 -> TASK-006 -> TASK-012 -> TASK-018 -> TASK-015
TASK-015 -> TASK-019/TASK-020/TASK-021 -> TASK-022/TASK-023
TASK-024 -> TASK-025 -> TASK-026 -> TASK-035 -> TASK-027/TASK-028
```
