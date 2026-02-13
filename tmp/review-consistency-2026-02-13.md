# AGTMUX ドキュメント整合性・網羅性レビュー結果

**レビュー対象**: agtmux-spec.md, implementation-plan.md, tasks.md, test-catalog.md
**レビュー日**: 2026-02-13
**レビュアー**: Opus Agent #2 (整合性・網羅性)

---

## 総合評価

4つのドキュメントは全体として高い整合性を保っており、FR/NFR からタスク、テストへのトレーサビリティが確立されている。フェーズ構成・依存関係・テストゲートの設計も概ね妥当である。主要な改善点は adapter registry の段階的導入戦略、TC-018 のフェーズゲート配置、daemon トランスポート層の決定に集約される。

---

## 1. ドキュメント間の整合性

### [Major] 1-1: TC-046 が Phase 2 ゲートに含まれず Phase 2.5 配置

`implementation-plan.md` セクション6「Dependency and Sequence Rules」に記載:
> No adapter expansion before Phase 2 gate bundle (`TC-033`, `TC-034`, `TC-035`, `TC-047`, `TC-048`, `TC-052`) is green.

一方、`test-catalog.md` の Phase 2 close ゲート:
> Phase 1.5 bundle + TC-033, TC-034, TC-035, TC-047, TC-048, TC-052

これは一致しているが、**TC-046（Adapter registry extensibility）が Phase 2 ゲートに含まれていない** 点が問題。TC-046 は TASK-035（Phase 2、adapter registry capability-driven dispatch）に紐づいており、Phase 2.5 の前提条件となるべきだが、Phase 2 close ゲートではなく Phase 2.5 close ゲートに配置されている。adapter registry がなければ Copilot/Cursor adapter を追加できないため、TC-046 は Phase 2 close ゲートに含めるべきである。

### [Minor] 1-2: implementation-plan Phase 0 の exit criteria と test-catalog Phase 0 ゲートの差分

`implementation-plan.md` Phase 0 exit criteria には以下が明記されている:
> Security/performance baseline tests are green (`TC-011`, `TC-012`, `TC-013`).
> Target execution and topology observation tests are green (`TC-040`, `TC-041`).

`test-catalog.md` Phase 0 close ゲートには TC-049, TC-050 も含まれているが、implementation-plan 側の exit criteria にはこれらへの言及がない。implementation-plan に列挙したテストが部分的であることで、読み手に「TC-049, TC-050 は本当に Phase 0 で必要なのか」という混乱を招く。

### [Minor] 1-3: spec Phase 1 の exit criteria に TC-018（エラーエンベロープ）言及なし

`test-catalog.md` Phase 1 close ゲートには TC-018 が含まれている。TC-018 は「API/CLI error envelope shape」のテストで、TASK-023（Phase 1.5）に紐づいている。Phase 1.5 のタスクに紐づくテストが Phase 1 ゲートに入っている点が不整合。TASK-023 は Phase 1.5 であるため、TC-018 も Phase 1.5 close ゲートに移すか、TASK-023 を Phase 1 に前倒しすべきである。

### [Major] 1-4: TASK-027 / TASK-028 の依存に TASK-035 があるが adapter registry の Phase 配置が遅い

TASK-027（Copilot CLI adapter、Phase 2.5）および TASK-028（Cursor CLI adapter、Phase 2.5）は `Depends On: TASK-025, TASK-026, TASK-035` と記載されている。TASK-035（adapter registry capability-driven dispatch）は Phase 2 のタスクであり、フェーズ的には先行するので依存関係自体は正しい。しかし、adapter registry は本来、Claude/Codex adapter の実装（Phase 1）でも活用されるべき基盤であり、Phase 0 または Phase 1 に前倒しが適切ではないか検討の余地がある。現状では Phase 1 の Claude/Codex adapter は registry を経由せず直接実装される設計となっており、Phase 2 でリファクタが必要になる可能性がある。

### [Major] 1-5: spec の `adapters` テーブルに対応するタスクの欠如

spec セクション 7.3 のデータモデルに `adapters` テーブルが定義されている（`adapter_name`, `agent_type`, `version`, `capabilities`, `enabled`, `updated_at`）。しかし tasks.md には `adapters` テーブルの作成・管理に直接対応するタスクが明示的に存在しない。TASK-001（base SQLite migrations）の「all core tables」に含まれると推測されるが、TASK-035（adapter registry）が Phase 2 であることから、`adapters` テーブルは Phase 0 の migration で作成するのか、Phase 2 で追加するのか曖昧である。

---

## 2. 仕様の網羅性

### [Major] 2-1: spec FR-11 (Adapter Registry) の Phase 0/1 での実装タスク不在

FR-11:「Provide adapter registry so new agent CLIs can be added without changing core state engine」は仕様に明記されているが、TASK-035（Phase 2）まで実装されない。Phase 1 で Claude/Codex adapter を実装する際に adapter 登録・選択メカニズムの基本形が必要となるはずだが、Phase 1 にはこれに相当するタスクがない。

### [Minor] 2-2: spec セクション 7.9 API v1 の Target 管理エンドポイントの欠如

spec セクション 7.9 の API v1 には Read endpoints（`GET /v1/panes`, `/v1/windows`, `/v1/sessions`, `/v1/watch`）と Write endpoints（`POST /v1/actions/*`）が定義されているが、Target 管理（`add`, `connect`, `list`, `remove`）に対応する API エンドポイントが定義されていない。CLI の `agtmux target` サブコマンド群は spec セクション 7.5 に定義されているが、これが daemon API 経由なのか CLI 直接操作なのかが明確でない。将来の macOS app からも target 管理が必要になる可能性がある。

### [Minor] 2-3: spec の `skew_budget` 設定に対応するタスクの欠如

spec セクション 7.2.2 に `skew_budget` (デフォルト10秒、configurable) が定義されているが、configuration メカニズム自体のタスクが tasks.md に存在しない。同様に、`completed -> idle` の demotion 時間（120秒、configurable）や reconciler の scan interval（2秒）など、configurable とされているパラメータの設定基盤タスクがない。

### [Minor] 2-4: spec の `--needs-action` フィルタに対するテスト不在

spec セクション 7.5 の `agtmux list panes` コマンドに `--needs-action` フィルタが定義されている。これは `waiting_input` や `waiting_approval` 状態のペインを素早くフィルタするための重要な UX 機能だが、test-catalog にこのフィルタの動作を検証する明示的なテストケースがない。TC-048（List/watch filter and sort stability）に含まれうるが、明示されていない。

### [Minor] 2-5: `kill` コマンドの `--yes` 確認プロンプトのタスク・テスト不在

spec セクション 7.7 に「confirmation prompt is required by default; `--yes` skips confirmation」と記載されているが、確認プロンプトの実装タスクやテストケースが明示されていない。`agtmux target remove <name> [--yes]` も同様。

---

## 3. テストカバレッジ

### [Major] 3-1: TC-044（Visibility latency benchmark）がどのタスクからも参照されていない

TC-044 は Phase 1 close ゲートに含まれているが、tasks.md のどのタスクの Test IDs カラムにも TC-044 が記載されていない。このテストに責任を持つタスクが不明確であり、実装漏れのリスクがある。NFR-1 に対応するパフォーマンステストであり、ベンチマーク基盤の構築タスクが必要。

### [Minor] 3-2: adapter 固有の capabilities テスト不在

spec セクション 7.2.3 では adapter が `supports_waiting_approval`, `supports_waiting_input`, `supports_completed` 等の capabilities を宣言すると定義されている。Codex は `waiting_approval` をサポートし、Claude は `waiting_input` をサポートするなど、adapter ごとの capabilities 差異がステートエンジンの挙動に影響するが、各 adapter の capabilities 宣言が正しく機能するかの明示的テストケースがない。

### [Minor] 3-3: `E_TARGET_UNREACHABLE` エラーコードの専用テスト不在

spec セクション 7.9 に `E_TARGET_UNREACHABLE` がエラーコードとして定義されている。TC-023（Partial-result envelope）や TC-043（Aggregated multi-target semantics）で間接的にテストされうるが、target への接続失敗時にこのエラーコードが適切に返される専用テストがない。

### [Minor] 3-4: `event_inbox` の `pending_bind` TTL 満了テストの粒度不足

TC-009 が「no candidate / ambiguous / TTL expiry」の3パターンをカバーすると記載されているが、1つのテストIDに3つの独立したシナリオが詰め込まれている。TTL 満了は時間依存の挙動であり、独立したテストケースとして分離すべき。

---

## 4. タスクの依存関係

### [Minor] 4-1: TASK-009（target manager）が TASK-031 に依存するが TASK-001 に直接依存していない

TASK-009（Phase 1、target manager commands）は `Depends On: TASK-031` としている。TASK-031 は TASK-001 に依存しているため推移的に TASK-001 にも依存しているが、target manager は `targets` テーブルを使用するため、TASK-001（SQLite migrations）への直接依存も明示したほうが明確。

### [Info] 4-2: TASK-012 が TASK-010, TASK-011（adapter）に依存していない

TASK-012（API v1 read endpoints、Phase 1）は `Depends On: TASK-004, TASK-006` となっている。API v1 read endpoints は adapter から投入されたデータを読み出すため、Claude/Codex adapter（TASK-010, TASK-011）が完了していなくても API 層は実装可能という設計判断と思われる。フェーズ内の並列実装を意図したものであれば妥当だが、integration テスト時には adapter が必要になる点に注意が必要。

### [Minor] 4-3: 即時スプリント候補に TASK-007, TASK-008, TASK-038 が含まれていない

tasks.md セクション2「Immediate Sprint Candidates」には Phase 0 タスクの TASK-001〜006, TASK-031, TASK-032 が挙がっているが、同じ Phase 0 の TASK-007（payload redaction）、TASK-008（index set）、TASK-038（duplicate storm hardening）が含まれていない。これらは P1 優先度であるため意図的だが、Phase 0 close ゲートには TC-011, TC-012, TC-013, TC-049, TC-050 が含まれており、これらのタスクが完了しないと Phase 0 をクローズできない。スプリント計画に含めるか、第2スプリントとして明示すべき。

---

## 5. バージョン・数値の整合性

### [Major] 5-1: TC-018 のフェーズ配置の不整合

前述（指摘 1-3）と重複するが、数値の観点から再掲。

- TASK-023（Phase **1.5**）の Test IDs に TC-018 が含まれている。
- test-catalog の Phase **1** close ゲートに TC-018 が含まれている。

タスクのフェーズ（1.5）とテストのゲートフェーズ（1）が一致していない。Phase 1 close 時点で TASK-023 はまだ未完了のはずであり、TC-018 を Phase 1 ゲートで要求するのは矛盾する。

### [Info] 5-2: 全テストIDの連番確認

test-catalog では TC-001 から TC-053 まで定義されている。連番は全て埋まっており、欠番はない。tasks.md から参照されるテストIDも全て test-catalog に存在する。ただし前述の通り TC-044 は tasks.md のどのタスクからも参照されていない。

### [Info] 5-3: フェーズ番号は全ドキュメント間で整合

Phase 0, 1, 1.5, 2, 2.5, 3 の区分は4つの全ドキュメントで一貫している。不整合なし。

---

## 6. 未定義・曖昧な記述

### [Minor] 6-1: spec の `connection_ref` の具体的な格納形式が未定義

spec セクション 7.3.1 に「`targets.connection_ref` MUST store only non-secret references (for example ssh host alias)」とあるが、SSH接続に必要な情報（ポート番号、ユーザ名、鍵ファイルパス等）の取り扱いが未定義。ssh host alias は `~/.ssh/config` 側の設定を前提とするのか、それとも追加の接続パラメータを持つのかが曖昧。

### [Major] 6-2: daemon のトランスポート層が未決定

spec セクション 7.9 に API v1 のエンドポイント（`GET /v1/panes` 等）が定義されているが、これが HTTP/REST なのか Unix Domain Socket なのか gRPC なのか、トランスポート層が明記されていない。実装計画にも具体的なトランスポートの選択に関するタスクがない。これは Phase 0 の daemon boundary 実装（TASK-031）に直接影響する重要な未決事項。

### [Minor] 6-3: spec Open Questions が2件残存

spec セクション 10 に2つの Open Questions が残っている:
1. Gemini のネイティブイベントフック提供時のデフォルトインジェストパス切り替え
2. 大規模環境での `--all-targets` デフォルトの是非

これらは Phase 2 以降に影響するため即座の解決は不要だが、Phase 2 開始前に決定すべきことを implementation-plan に明記すべき。

### [Minor] 6-4: `watch` コマンドの `--interval` のデフォルト値未定義

spec セクション 7.5 の `agtmux watch` コマンドに `--interval <duration>` オプションがあるが、デフォルト値が記載されていない。reconciler の scan interval（2秒）との関係も不明確。

### [Minor] 6-5: `view-output` の `--lines` デフォルト値未定義

spec セクション 7.5 の `agtmux view-output <ref> [--lines <n>]` の `--lines` のデフォルト値が未定義。「bounded by line limit」とあるが具体的な数値がない。

### [Minor] 6-6: action snapshot の `expires_at` のデフォルトTTLが未定義

spec セクション 7.7 で action snapshot に `expires_at` フィールドがあり、期限切れ時は `E_SNAPSHOT_EXPIRED` で拒否されると定義されている。しかしスナップショットのデフォルト有効期間（TTL）が明記されていない。

---

## レビューサマリー

| 重要度 | 件数 |
|--------|------|
| Critical | 0 |
| Major | 6 |
| Minor | 14 |
| Info | 3 |
| **合計** | **23** |

### Major 指摘一覧

| # | 指摘 | 対処推奨 |
|---|------|----------|
| 1-1 | TC-046 が Phase 2 ゲートに含まれず Phase 2.5 配置 | TC-046 を Phase 2 close ゲートに移動 |
| 1-4 | adapter registry (TASK-035) の Phase 配置が遅い | Phase 1 に adapter registry の基本形タスクを追加するか、Phase 2 でのリファクタコストを明示的に受容 |
| 1-5 | `adapters` テーブル作成の責務タスクが曖昧 | TASK-001 の受入基準に `adapters` テーブルを明示するか、専用タスクを追加 |
| 2-1 | FR-11 の Phase 0/1 実装不在 | 指摘 1-4 と同根。adapter registry 基盤の前倒しを検討 |
| 3-1 | TC-044 がどのタスクからも未参照 | パフォーマンスベンチマーク基盤タスクを追加し TC-044 を紐づけ |
| 5-1 | TC-018 のフェーズゲート(Phase 1) とタスクフェーズ(1.5) の不整合 | TC-018 を Phase 1.5 ゲートに移動するか、TASK-023 を Phase 1 に前倒し |
| 6-2 | daemon トランスポート層が未決定 | spec に transport の選択を追記し、TASK-031 の受入基準に反映 |
