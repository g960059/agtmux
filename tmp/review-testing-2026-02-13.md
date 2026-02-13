# AGTMUX テスト・品質保証レビュー報告書

**レビュー対象**: agtmux-spec.md, implementation-plan.md, tasks.md, test-catalog.md (v0.5 Draft)
**レビュー日**: 2026-02-13
**レビュアー**: Opus Agent #4 (テスト・品質保証)

---

## 総合評価

ドキュメント全体の品質は高く、テスト戦略・フェーズゲート・トレーサビリティにおいて、多くのプロジェクトの平均を大きく上回る設計がなされている。特に、テストID <-> タスクID <-> FR/NFR の三方向トレーサビリティ、フェーズゲートバンドルの累積構造、Property Test の採用は優れた設計判断である。ただし、以下に述べる通り、いくつかの重要な欠落と曖昧さが存在する。

---

## 1. テスト戦略の妥当性

### [Major] M-01: テストピラミッドの定量的バランスが未定義

`test-catalog.md` Section 1 で Unit / Property / Integration / Contract / E2E / Performance / Resilience の6レイヤーを定義しているが、各レイヤーの期待比率や目標件数が記載されていない。現行の TC-001 〜 TC-053 を分類すると以下のようになる:

| レイヤー | 件数 | 比率 |
|---|---|---|
| Unit | 2 (TC-004, TC-007) | 3.8% |
| Property | 1 (TC-006) | 1.9% |
| Integration | 18 | 34.0% |
| Contract | 12 | 22.6% |
| E2E | 13 | 24.5% |
| Performance | 2 (TC-013, TC-044) | 3.8% |
| Resilience | 3 (TC-034, TC-051, TC-052) | 5.7% |
| Security | 1 (TC-050) | 1.9% |
| Manual+CI | 2 (TC-038, TC-039) | 3.8% |

**Unit テストが全体の 3.8% (2件) しかない。** これはテストピラミッドの原則に著しく反している。パーサーロジック、状態遷移ロジック、優先度比較、エンコーディング/デコーディング、dedupe_key 生成関数、effective_event_time 計算など、Unit テストで高速にフィードバックすべきロジックが多数存在するにもかかわらず、それらがカタログに含まれていない。

**推奨**: Unit テスト対象を網羅的に洗い出し、少なくとも 15-20 件の Unit テストケースを追加すべき。具体的には:
- 状態優先度比較ロジック (precedence)
- `effective_event_time` 計算 (skew_budget 内/外)
- `dedupe_key` 生成関数
- RFC3986 percent-encoding/decoding
- `runtime_id` の sha256 導出
- `completed -> idle` 降格タイマーロジック
- アクションリファレンス文法パーサー (BNF に基づくバリデーション)
- 各アダプターの `Normalize(signal)` 関数

### [Minor] M-02: テスト自動化方針における Manual+CI の定義が曖昧

TC-038, TC-039 が `Manual+CI` と記載されているが、Manual 部分の具体的なテスト手順書、テスト環境要件、合格判定の基準が未定義。Phase 3 の macOS アプリはレビュー可能な UI テストフレームワーク (XCUITest 等) を利用する想定があるのか不明。

**推奨**: Manual テストの実行手順テンプレートと、可能な限り自動化する範囲の明確化を追加すべき。

### [Info] I-01: Property テストの採用は適切

TC-006 (Ordering determinism) に Property テストを採用している点は、決定論的状態エンジンの品質保証として適切である。ただし、Property テストの対象を拡大すべき (後述 M-06)。

---

## 2. テストケースの品質

### [Major] M-03: テストケースの「シナリオ」と「Pass Criteria」の粒度が不均一

一部のテストケースは十分に具体的だが、多くのケースで Scenario / Pass Criteria が抽象的すぎる。例:

- **TC-010**: Scenario = "target down and stale signals", Pass Criteria = "state becomes `unknown/*`"
  - どの時間枠で unknown に収束すべきか? TTL 値は? "stale signals" の具体的条件は?

- **TC-014**: Scenario = "add/connect/list/remove targets", Pass Criteria = "all commands succeed with expected output"
  - "expected output" の定義がない。JSON 形式? テーブル形式? エラーケースは?

- **TC-034**: Scenario = "repeated target flaps", Pass Criteria = "no deadlock, recovers within SLO"
  - "SLO" が本ドキュメント内で未定義。NFR-1 の 2秒は visibility lag であり、reconnect recovery SLO は別物。flap の回数・間隔の具体的パラメータが不明。

- **TC-041**: Scenario = "target reconnect and pane churn", Pass Criteria = "topology converges without stale bleed"
  - "stale bleed" の定義が不明。定量的基準がない。

**推奨**: 各テストケースに対し、(a) 具体的な前提条件/テストデータ、(b) 具体的な操作手順またはテストコードの擬似的な記述、(c) 定量的または判定可能な Pass Criteria を記述すべき。少なくとも Critical Path に属するテスト (Phase 0/1 ゲート) は優先的に詳細化すべき。

### [Minor] M-04: テストフィクスチャの内容が未定義

Section 5 "Test Data and Fixtures" にフィクスチャのカテゴリは列挙されているが、具体的なフィクスチャファイルの構造・フォーマット・サンプルが存在しない。「Synthetic ordered/disordered event fixtures per adapter」が JSON なのか、YAML なのか不明。

**推奨**: フィクスチャのスキーマ定義とサンプルデータを1件以上提示し、テスト実装時の曖昧さを排除すべき。

---

## 3. 境界値・異常系テスト

### [Critical] C-01: 境界値テストの体系的な欠落

テストカタログ全体を通じて、境界値テストが著しく不足している。以下の境界値条件に対するテストケースが存在しない:

1. **pane_epoch の最大値/オーバーフロー**: epoch がどこまで増加した場合に問題が生じるか
2. **session_name のエンコーディング境界**: 空文字列、最大長文字列、マルチバイト文字 (日本語/絵文字)、制御文字を含むセッション名
3. **source_seq のオーバーフロー/リセット**: int64 の最大値、0リセット後の挙動
4. **skew_budget の境界**: ちょうど 10 秒 (等号条件)、10.001 秒、9.999 秒
5. **bind_window の境界**: ちょうど 5 秒 (デフォルト bind window)
6. **pending-bind TTL の期限切れ直前/直後**
7. **completed -> idle 降格のちょうど 120 秒境界**
8. **view-output の line limit = 0、1、最大値**
9. **watch cursor の stream_id:sequence の形式不正パターン** (`:` なし、sequence が負、sequence が非数値)
10. **同時接続ターゲット数の上限** (3ターゲットのベンチマークはあるが、上限テストがない)
11. **イベント ingest rate が極端に高い場合** (burst 1000 events/sec など)
12. **SQLite の同時書き込み上限** (WAL モード前提か?)

**推奨**: 上記の境界値条件について、最低でも重要度の高い 5-6 件をテストカタログに追加すべき。特に項目 3, 4, 5, 7 は状態エンジンの正確性に直結するため Critical。

### [Major] M-05: 異常系テストの網羅性不足

以下の異常系シナリオに対するテストケースが欠落している:

1. **SQLite DB の破損・マイグレーション不整合時の起動動作**
2. **tmux サーバーが未起動の状態での daemon 起動**
3. **SSH 接続の認証失敗時の target manager の挙動**
4. **アダプターが panic/crash した場合の daemon 全体への影響**
5. **event_inbox が大量の pending_bind で溢れた場合のメモリ/性能影響**
6. **同一 pane に対する複数アダプターの同時イベント送信 (agent_type の衝突)**
7. **daemon の graceful shutdown 中に到着したイベントの処理**
8. **ディスク容量不足時の SQLite 書き込みエラーのハンドリング**
9. **kill 実行後のイベントレース** (kill 送信 -> 状態変更イベント到着前に次のコマンド実行)
10. **不正な JSON payload を持つイベントの ingestion**

**推奨**: 少なくとも項目 1, 2, 4, 7, 9 をテストカタログに追加すべき。

### [Major] M-06: Property テストの対象が狭すぎる

現状 Property テストは TC-006 (Ordering determinism) のみだが、以下も Property テスト候補として適切:

- **状態収束の不変条件**: 任意のイベント順序で最終状態が同一
- **べき等性の不変条件**: 同一イベントの N 回適用と 1 回適用が同一結果
- **runtime guard の不変条件**: stale runtime_id を含むイベントが状態を変更しないこと
- **pending-bind の不変条件**: bind 完了後に同一イベントを再 bind しないこと

**推奨**: 上記の不変条件を Property テストとして追加し、テストの信頼性を高めるべき。

---

## 4. テスト実行環境

### [Critical] C-02: tmux 依存のテスト実行環境に関する考慮が不足

テストカタログの E2E テスト (TC-014, TC-019, TC-022, TC-024, TC-027, TC-028, TC-029, TC-030, TC-041) は tmux の実行を前提としているが、以下が未定義:

1. **CI 環境での tmux の可用性**: GitHub Actions / GitLab CI 等で tmux がプリインストールされているか、Docker イメージに含めるか
2. **tmux のバージョン要件**: tmux のバージョンによって `send-keys` や `capture-pane` の動作が異なる可能性がある。最小サポートバージョンの明記がない
3. **ヘッドレス環境での tmux 起動**: CI 環境に TTY がない場合の tmux サーバー起動方法 (`tmux new-session -d` 等)
4. **テスト間の tmux セッション分離**: 並列テスト実行時に tmux セッションが衝突しないための命名規則・クリーンアップ戦略
5. **SSH ターゲットの E2E テスト**: TC-040, TC-041 で SSH ターゲットが必要だが、CI 環境での SSH テストサーバーの構築方法が未定義
6. **OS 依存**: macOS と Linux での tmux の動作差異 (特に `tmux_server_boot_id` の取得方法)

**推奨**: テスト実行環境の前提条件を明文化し、以下を定義すべき:
- CI 用 Docker イメージの tmux バージョン要件
- tmux モック/スタブ戦略 (Integration テストではモック、E2E では実 tmux)
- SSH テスト用のローカル sshd またはコンテナベースの分離環境
- テスト並列実行時のリソース分離ポリシー

### [Major] M-07: Nightly テストの実行基盤が未定義

TC-012, TC-013, TC-034, TC-044, TC-052 が `Nightly` に分類されているが、Nightly テストを実行するインフラ (スケジューラ、実行環境、結果通知、アーティファクト保存) が定義されていない。

**推奨**: Nightly テストの実行基盤要件を明文化し、最低限以下を記載すべき:
- 実行スケジュール (何時に実行するか)
- 実行環境のスペック (特に TC-044 のパフォーマンスベンチマークに必要な CPU/メモリ)
- 失敗時の通知フロー
- アーティファクトの保存期間

---

## 5. テストゲート

### [Minor] M-08: Phase 0 ゲートに TC-041 が含まれ、Phase目的に対して過剰な可能性

Phase 0 ゲートバンドルには **17件** のテストが含まれており、Phase 0 のタスク数 (TASK-001〜008, TASK-031, TASK-032, TASK-038 = 11件) に対してやや過大。TC-041 (Multi-target topology observer, E2E) は Phase 0 ゲートに含まれているが、マルチターゲットの E2E テストを Phase 0 で要求することは、Phase 0 の目的 ("Core Runtime") に対して過剰ではないか検討の余地がある。

**推奨**: TC-041 を Phase 1 に移動するか、Phase 0 では単一ターゲットのトポロジーテストに限定する軽量版を定義し、マルチターゲットは Phase 1 に持ち越すことを検討すべき。

### [Minor] M-09: カバレッジ目標が未定義

テストゲートにおいてカバレッジ目標 (ライン/ブランチカバレッジの数値目標) が全く記載されていない。テストが全て Green でも、実際のコードカバレッジが低い場合、品質保証としては不十分。

**推奨**: 以下のカバレッジ目標を設定すべき:
- 状態エンジン (core): ブランチカバレッジ 90% 以上
- アダプター: ブランチカバレッジ 80% 以上
- CLI/API ハンドラー: ラインカバレッジ 70% 以上
- 全体: ラインカバレッジ 75% 以上

### [Info] I-02: ゲートの累積構造は適切

Phase N+1 のゲートが Phase N のバンドルを包含する累積構造は、回帰テストの自動的な保証として優れた設計。

---

## 6. 回帰テスト

### [Major] M-10: 回帰テスト戦略の明文化が不足

ゲートバンドルの累積構造により暗黙的な回帰テストは担保されているが、以下が明文化されていない:

1. **回帰テストの実行タイミング**: PR ごとに全フェーズの累積テストを実行するのか、当該フェーズのバンドルのみか
2. **回帰テスト失敗時の対処フロー**: 過去フェーズのテストが失敗した場合の escalation path
3. **スキーマ変更時の回帰影響分析**: SQLite スキーマのマイグレーションが既存テストに与える影響の検証方法
4. **アダプター追加時の既存アダプターへの回帰**: Phase 2.5 で Copilot/Cursor を追加する際、Claude/Codex/Gemini アダプターの回帰を保証する仕組み

**推奨**: 回帰テストポリシーを明文化し、以下を定義すべき:
- CI での回帰テスト実行範囲 (全バンドル vs 当該フェーズ)
- 回帰テスト失敗時のブロッキングルール
- アダプター追加時の共有コントラクトテストスイートの実行義務

### [Info] I-03: Contract テストによるスキーマ回帰防止は適切

TC-035 (JSON schema compatibility) と TC-045 (Watch JSONL schema compatibility) が "compare schema snapshots across commits" としている点は、スキーマの意図しない破壊的変更を検知する回帰テストとして有効。ただし、スキーマスナップショットの管理方法 (リポジトリ内のゴールデンファイルか、ツールベースか) が未定義。

---

## 7. パフォーマンステスト

### [Major] M-11: パフォーマンステスト計画の網羅性不足

パフォーマンステストは TC-013 (Index baseline utility) と TC-044 (Visibility latency benchmark) の 2件のみ。以下のパフォーマンス観点が欠落している:

1. **メモリ使用量テスト**: daemon のメモリ使用量の上限・リーク検知。NFR-3 "Low overhead on host CPU/memory" に対応するテストがない
2. **CPU 使用率テスト**: reconciler の定期実行 (2秒間隔) が CPU に与える影響の定量測定
3. **SQLite DB サイズの増加テスト**: retention purge が正しく機能し、DB サイズが無限に成長しないことの検証
4. **大量ペインでのスケーラビリティテスト**: ベンチマークプロファイルは 60 panes だが、200 panes や 500 panes でのスケーラビリティは未検証
5. **watch ストリームの負荷テスト**: 複数クライアントが同時に watch を購読した場合のパフォーマンス
6. **イベントバースト時のバックプレッシャー挙動**: event_inbox が急増した場合の処理遅延の測定

**推奨**: 少なくとも項目 1 (メモリリーク検知), 3 (DB サイズ成長), 4 (大量ペインスケーラビリティ) をパフォーマンステストとして追加すべき。

### [Minor] M-12: パフォーマンスベンチマークのベースライン管理が未定義

TC-044 のベンチマークプロファイルは定義されているが、パフォーマンスの回帰を検知するためのベースライン管理方法が不明。ベンチマーク結果を時系列で追跡し、有意な劣化を検知する仕組みが必要。

**推奨**: ベンチマーク結果のストレージ・可視化・アラートの方針を定義すべき。

---

## 追加指摘事項

### [Critical] C-03: テスト技術スタックの未定義

テストカタログには「何をテストするか」は記載されているが、「どの技術でテストするか」が一切記載されていない。以下が不明:

1. **実装言語**: Go? Rust? TypeScript? テストフレームワークの選定が不可能
2. **テストフレームワーク**: Go なら `testing` + `testify`、Property テストなら `rapid` / `gopter` 等
3. **モック/スタブ戦略**: tmux コマンドのモック方法、SSH のモック方法
4. **CI ツール**: GitHub Actions? GitLab CI? Jenkins?
5. **パフォーマンステストツール**: `go test -bench`? カスタムベンチマーク?

**推奨**: テスト技術スタックを明文化し、最低限以下を定義すべき:
- テストフレームワーク
- モック/スタブライブラリ
- CI/CD パイプライン構成
- パフォーマンステストツール

### [Major] M-13: tasks.md Phase 1 内部のタスク実行順序ガイダンスがない

TASK-018 (Implement shared `actions` write path with idempotency) は Phase 1, P0 だが、TASK-015 (Implement attach action with snapshot validation) が TASK-018 に依存している。Phase 1 の中で TASK-018 -> TASK-015 という順序が暗黙的に要求されているにもかかわらず、Sprint Candidates (Section 2) には Phase 0 のタスクしかリストされておらず、Phase 1 内部の実行順序ガイダンスがない。

**推奨**: Phase 1 のタスク実行順序のガイダンスを tasks.md に追加すべき。

### [Major] M-14: セキュリティテストの不足

TC-011 (Payload redaction) と TC-050 (Debug raw payload prohibition) の2件しかセキュリティテストが存在しない。以下が欠落している:

1. **SSH credential の安全な取り扱いテスト**: `connection_ref` に SSH 秘密鍵パスや生パスワードが含まれないことの検証
2. **API エンドポイントの認証・認可テスト**: daemon API v1 に認証メカニズムがあるか、ローカルソケットのみか
3. **インジェクション攻撃テスト**: `send --text` で悪意あるシェルコマンドが注入されないことの検証 (tmux send-keys 経由)
4. **SQLite DB ファイルのパーミッションテスト**: DB ファイルが適切な権限で作成されることの検証

**推奨**: 少なくとも項目 1, 3 をセキュリティテストとして追加すべき。

### [Minor] M-15: implementation-plan.md と test-catalog.md のゲート参照の軽微な不一致

implementation-plan.md Section 6 に "No adapter expansion before Phase 2 gate bundle (`TC-033`, `TC-034`, `TC-035`, `TC-047`, `TC-048`, `TC-052`) is green" とあるが、test-catalog.md の Phase 2 close バンドルは "Phase 1.5 bundle + TC-033, TC-034, TC-035, TC-047, TC-048, TC-052" と定義されている。implementation-plan.md の記載は Phase 2 固有の追加分のみを列挙しているため一見矛盾に見えるが、実質的には Phase 1.5 バンドルの通過が Phase 2 の前提条件として暗黙的に含まれている。この暗黙性は混乱を招く可能性がある。

**推奨**: implementation-plan.md のゲート参照を「Phase 2 close バンドル (Phase 1.5 バンドル含む) が全て green」と明記すべき。

### [Minor] M-16: テストカタログに「負荷テスト」カテゴリが存在しない

Section 1 の Test layers に "Performance and resilience tests" はあるが、「負荷テスト (Load Test)」としての明示的なカテゴリがない。TC-049 (Duplicate-storm convergence) は暗黙的に負荷テストだが、明示的な分類がないため、レイヤーごとの管理が曖昧になる。

---

## 指摘事項サマリー

| 重要度 | ID | カテゴリ | 概要 |
|---|---|---|---|
| **Critical** | C-01 | 境界値テスト | 境界値テストの体系的な欠落 (12項目以上の境界条件が未カバー) |
| **Critical** | C-02 | テスト実行環境 | tmux/SSH 依存のCI実行環境に関する考慮が全面的に不足 |
| **Critical** | C-03 | テスト戦略 | テスト技術スタック (言語/フレームワーク/CI) が未定義 |
| **Major** | M-01 | テスト戦略 | Unit テストが全体の 3.8% と著しく不足 (テストピラミッド違反) |
| **Major** | M-03 | テストケース品質 | シナリオと Pass Criteria の粒度が不均一で抽象的なケースが多い |
| **Major** | M-05 | 異常系テスト | 異常系シナリオが 10 件以上カバーされていない |
| **Major** | M-06 | Property テスト | Property テストの対象が 1 件のみで不十分 |
| **Major** | M-07 | テスト実行環境 | Nightly テストの実行基盤が未定義 |
| **Major** | M-10 | 回帰テスト | 回帰テスト戦略の明文化が不足 |
| **Major** | M-11 | パフォーマンステスト | メモリ/CPU/スケーラビリティのパフォーマンステストが欠落 |
| **Major** | M-13 | タスク管理 | Phase 1 内部のタスク実行順序ガイダンスがない |
| **Major** | M-14 | セキュリティテスト | SSH credential / API認証 / インジェクション攻撃のテストが欠落 |
| **Minor** | M-02 | テスト戦略 | Manual+CI テストの具体的手順が未定義 |
| **Minor** | M-04 | テストケース品質 | テストフィクスチャの構造/フォーマットが未定義 |
| **Minor** | M-08 | テストゲート | Phase 0 ゲートに E2E (TC-041) が含まれ、Phase目的に対して過剰な可能性 |
| **Minor** | M-09 | テストゲート | コードカバレッジ目標が未定義 |
| **Minor** | M-12 | パフォーマンステスト | パフォーマンスベースラインの時系列管理が未定義 |
| **Minor** | M-15 | ドキュメント整合性 | implementation-plan.md のゲート参照で累積バンドルの暗黙性 |
| **Minor** | M-16 | テスト分類 | 負荷テストカテゴリの欠落 |
| **Info** | I-01 | テスト戦略 | Property テストの採用は適切 |
| **Info** | I-02 | テストゲート | ゲートの累積構造は適切 |
| **Info** | I-03 | 回帰テスト | スキーマスナップショットベースの回帰防止は有効 |

**統計**: Critical 3件 / Major 10件 / Minor 8件 / Info 3件

---

## 優先対応推奨

Phase 0 コーディング開始前に以下の 5 項目を必ず解決すべき:

1. **C-03**: テスト技術スタックを確定し、テストフレームワーク/CI パイプラインの基盤を構築する
2. **C-02**: CI 環境での tmux/SSH テスト実行方法を確立し、Docker イメージ等の基盤を整備する
3. **C-01**: 状態エンジンの境界値テスト (skew_budget, bind_window, completed->idle タイマー) を最低 5 件追加する
4. **M-01**: Unit テストケースを 15 件以上洗い出し、テストカタログに追加する
5. **M-09**: カバレッジ目標を設定し、CI パイプラインにカバレッジレポートを組み込む
