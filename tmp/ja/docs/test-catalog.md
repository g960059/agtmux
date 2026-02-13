# AGTMUX テストカタログ (v0.5)

Date: 2026-02-13
Status: Draft
Source Spec: `docs/agtmux-spec.md`
Plan Reference: `docs/implementation-plan.md`
Task Reference: `docs/tasks.md`

## 1. テスト戦略

テストレイヤー:

- パーサー/比較器/バリデーションロジックの Unit テスト。
- 順序決定性およびリプレイ不変条件の Property テスト。
- DB + デーモン + アダプターの Integration テスト。
- API/CLI スキーマおよびエラーエンベロープの Contract テスト。
- マルチターゲットワークフローの End-to-end テスト。
- レイテンシおよび部分障害時の挙動に関する Performance / Resilience テスト。

## 2. Contract テストマトリクス

| テストID | レイヤー | 契約 | 要件元 | シナリオ | 合格基準 | 自動化 |
| --- | --- | --- | --- | --- | --- | --- |
| TC-001 | Integration | ベースマイグレーションの正確性 | FR-3 | 新規マイグレーションの適用とロールバック | スキーマの適用/ロールバックが正常に完了する | CI |
| TC-002 | Integration | コアテーブル制約 | FR-14, FR-15 | PK/UNIQUE/FK 制約の検証 | 不正な書き込みが拒否される | CI |
| TC-003 | Integration | アクティブランタイムの一意性 | FR-13 | 同一ペインへの同時ランタイム挿入 | アクティブなランタイムは最大1つ | CI |
| TC-004 | Unit | ランタイムID生成 | FR-13 | エポック増分トリガーとランタイムロールオーバー | 古いランタイムが拒否される | CI |
| TC-005 | Integration | 古いランタイムのガード | FR-13, FR-15 | ペイン再利用後の旧ランタイムに対するアクション | `E_RUNTIME_STALE` | CI |
| TC-006 | Property | 順序決定性 | FR-14, NFR-8 | 同一イベントセットを繰り返しシャッフル | 最終状態ハッシュが同一 | CI |
| TC-007 | Unit | 重複排除の挙動 | FR-14, NFR-4 | 重複イベントの送信 | 論理的に1回のみ適用 | CI |
| TC-008 | Integration | 保留バインドの安全な解決 | FR-14 | ヒントが一致する候補が存在 | inbox が `bound` に遷移する | CI |
| TC-009 | Integration | 保留バインドの拒否パス | FR-14 | 候補なし / 曖昧 / TTL 期限切れ | `dropped_unbound` + reason_code | CI |
| TC-010 | Integration | unknown 安全な収束 | FR-8, NFR-2 | ターゲットダウンおよび古いシグナル | 状態が `unknown/*` になる | CI |
| TC-011 | Integration | ペイロードの秘匿化 | NFR-9 | 機密ペイロードサンプルの取り込み | 保存されたペイロードが秘匿化されている | CI |
| TC-012 | Integration | 保持期間パージの安全性 | NFR-9 | 保持ジョブの実行 | 期限切れの行が削除され、残存データの整合性が維持される | Nightly |
| TC-013 | Performance | インデックスベースラインの有用性 | NFR-1, NFR-3 | ホットクエリのプロファイリング | インデックスベースの実行計画、許容範囲のレイテンシ | Nightly |
| TC-014 | E2E | ターゲットマネージャーの基本フロー | FR-10 | ターゲットの追加/接続/一覧/削除 | すべてのコマンドが期待通りの出力で成功する | CI |
| TC-015 | Integration | Claude 状態正規化 | FR-1, FR-2 | フックイベントフィクスチャ | 正規状態が正しい | CI |
| TC-016 | Integration | Codex 状態正規化 | FR-1, FR-2 | notify/wrapper フィクスチャ | 正規状態が正しい | CI |
| TC-017 | Contract | API 読み取りスキーマ | FR-4, FR-6, FR-7, FR-9 | `/v1/panes\|windows\|sessions` のレスポンス（グルーピングおよびマルチターゲット集約を含む） | 必須フィールド、ID形状、グルーピング件数、集約セマンティクスが安定している | CI |
| TC-018 | Contract | API/CLI エラーエンベロープ形状 | FR-16 | 不正な ref/cursor/action | code/message/details スキーマが安定している | CI |
| TC-019 | E2E | Watch カーソル再開 | FR-4, FR-6 | 有効なカーソルによるストリーム再開 | ギャップなし、重複なし | CI |
| TC-020 | E2E | 期限切れカーソルでの Watch リセット | FR-4, FR-6 | 古いカーソルのリクエスト | reset+snapshot 動作が発行される | CI |
| TC-021 | Contract | list/watch の CLI/API パリティ | FR-4, FR-6 | CLI JSON と API JSON の比較 | セマンティクスの一致 | CI |
| TC-022 | E2E | Attach のフェイルクローズ | FR-5, FR-15 | 古いスナップショット/ランタイムでの Attach | リクエストが拒否される | CI |
| TC-023 | Contract | 部分結果エンベロープ | NFR-7 | 集約読み取り中に1つのターゲットが失敗 | `partial` および `target_errors` が存在する | CI |
| TC-024 | E2E | エンコード済みターゲットセッションのラウンドトリップ | FR-16 | `/`、`%`、スペースを含むセッション名 | フィルター/ref の解決が正しい | CI |
| TC-025 | Integration | アクションの冪等リプレイ | FR-15, NFR-4 | 同一 request_ref の再送信 | 同一の action_id と結果 | CI |
| TC-026 | Integration | 冪等性の競合 | FR-15, NFR-4 | 同一キーで異なるペイロード | `E_IDEMPOTENCY_CONFLICT` | CI |
| TC-027 | E2E | Send アクションモード | FR-5, FR-15 | text/stdin/key/paste フロー | 期待される tmux の挙動 + ガードチェック | CI |
| TC-028 | E2E | View-output の範囲制限 | FR-5 | 行数制限付きキャプチャ | 制限範囲内の出力が正しい | CI |
| TC-029 | E2E | Kill モード key | FR-5 | key モードによる INT | グレースフルな割り込みパスが動作する | CI |
| TC-030 | E2E | Kill モードのシグナル検証 | FR-5 | PID なしでの signal モード | `E_PID_UNAVAILABLE` | CI |
| TC-031 | Integration | アクション-イベントの相関 | FR-15 | アクション実行とイベント検査 | action_id で追跡可能 | CI |
| TC-032 | Contract | エラーコードマッピングの一貫性 | FR-16 | API + CLI のエラーシナリオ | 自動化のための安定したマッピング | CI |
| TC-033 | Integration | Gemini アダプターの収束 | FR-1, FR-2 | wrapper/parser フィクスチャ + 順序不整合 | 安定した正規状態への収束 | CI |
| TC-034 | Resilience | 再接続/バックオフの挙動 | NFR-1, NFR-7 | ターゲットの繰り返しフラップ | デッドロックなし、SLO 内で回復 | Nightly |
| TC-035 | Contract | JSON スキーマ互換性 | FR-6 | コミット間のスキーマスナップショット比較 | 後方互換な変更のみ | CI |
| TC-036 | Integration | Copilot アダプター Contract スイート | FR-12 | アダプター Integration フィクスチャ | コアエンジンに変更なし | CI |
| TC-037 | Integration | Cursor アダプター Contract スイート | FR-12 | アダプター Integration フィクスチャ | コアエンジンに変更なし | CI |
| TC-038 | E2E | macOS アプリの読み取りパリティ | Goal | アプリ画面と API v1 データの比較 | パリティが検証される | Manual+CI |
| TC-039 | E2E | macOS アプリのアクション安全性パリティ | Goal, FR-15 | 古いランタイムでのアプリアクション | CLI と同一のフェイルクローズ動作 | Manual+CI |
| TC-040 | Integration | TargetExecutor とデーモンの境界 | FR-9 | ローカル/SSH 混在ターゲットのデーモン UDS API 経由での読み書きフロー | すべてのターゲット操作が executor 境界を通過し、`/v1/health` 契約が安定している | CI |
| TC-041 | E2E | マルチターゲットトポロジオブザーバー | FR-3, FR-9 | ターゲット再接続とペインの増減 | 古い状態の漏出なくトポロジが収束する | CI |
| TC-042 | Contract | グルーピングとサマリーの正確性 | FR-7 | panes/windows/sessions のロールアップ | 件数と優先順位が正しい | CI |
| TC-043 | Contract | 集約マルチターゲットセマンティクス | FR-9, NFR-7 | 集約読み取り中のターゲット部分障害 | requested/responded/target_errors の整合性 | CI |
| TC-044 | Performance | 可視性レイテンシベンチマーク | NFR-1 | ベンチマークプロファイルトラフィック | 可視遅延 p95 <= 2秒かつベンチマーク成果物が出力される | Nightly |
| TC-045 | Contract | Watch JSONL スキーマ互換性 | FR-6 | コミット間の watch snapshot/delta スキーマ比較 | スキーマの互換性が維持される | CI |
| TC-046 | Integration | アダプターレジストリの拡張性 | FR-11, NFR-5 | レジストリを通じたモックアダプターの追加 | コアエンジンの変更が不要 | CI |
| TC-047 | Contract | アダプター契約バージョン互換性 | NFR-6 | アダプターのマイナーバージョンアップ | 後方互換な挙動が維持される | CI |
| TC-048 | Contract | list/watch のフィルターとソートの安定性 | FR-4, FR-7 | フィルター+ソートの組み合わせ | 決定論的な順序と安定したスキーマ | CI |
| TC-049 | Integration | 重複ストームの収束 | FR-14, NFR-4 | 重複/リトライバーストのリプレイ | 状態の分岐や二重適用がない | CI |
| TC-050 | Security | デバッグ時の生ペイロード禁止 | NFR-9 | シークレットに類似したペイロードでデバッグモードを実行 | 秘匿化されていないペイロードが SQLite に記録されない | CI |
| TC-051 | Resilience | デーモン再起動後の Watch 継続性 | FR-4, NFR-7 | アクティブな watch ストリーム中の再起動 | reset/snapshot により欠損なく再開される | CI |
| TC-052 | Resilience | SQLite busy/lock リカバリ | NFR-3, NFR-7 | ロック競合の注入 | リトライ/バックオフにより整合性が維持される | Nightly |
| TC-053 | Contract | 全エラーコードマトリクスの回帰テスト | FR-16 | 定義済み全エラーコードの列挙 | API/CLI コードマッピングが完全かつ安定している | CI |

## 3. フェーズゲートバンドル

| ゲート | 必須テスト |
| --- | --- |
| Phase 0 close | TC-001, TC-002, TC-003, TC-004, TC-005, TC-006, TC-007, TC-008, TC-009, TC-010, TC-011, TC-012, TC-013, TC-040, TC-041, TC-049, TC-050 |
| Phase 1 close | Phase 0 バンドル + TC-014, TC-015, TC-016, TC-017, TC-018, TC-019, TC-020, TC-021, TC-022, TC-023, TC-024, TC-042, TC-043, TC-044, TC-045, TC-051 |
| Phase 1.5 close | Phase 1 バンドル + TC-025, TC-026, TC-027, TC-028, TC-029, TC-030, TC-031, TC-032, TC-053 |
| Phase 2 close | Phase 1.5 バンドル + TC-033, TC-034, TC-035, TC-046, TC-047, TC-048, TC-052 |
| Phase 2.5 close | Phase 2 バンドル + TC-036, TC-037 |
| Phase 3 close | Phase 2.5 バンドル + TC-038, TC-039 |

## 4. ベンチマークプロファイル (NFR-1 向け)

可視性レイテンシゲートのデフォルトベンチマークプロファイル:

- ターゲット数: 3 (ホスト + SSH ターゲット2台)
- アクティブペイン数: 合計60
- イベント取り込みレート: 持続的に10イベント/秒
- バジェット: 可視遅延 p95 <= 2秒

## 5. テストデータとフィクスチャ

- アダプターごとの合成済み順序付き/順序不整合イベントフィクスチャ。
- ペイン再利用タイムラインを含む保留バインドの曖昧性フィクスチャ。
- 保持ウィンドウのカットオフを含むカーソル期限切れフィクスチャ。
- シークレットに類似したペイロード文字列を含むセキュリティフィクスチャ。
- クロックスキューのバリエーションを含むマルチターゲット再接続フィクスチャ。

## 6. 再現性ルール

- Property テストはランダムシードを固定し、失敗出力にそのシードを報告すること。
- 時間依存テストは、明示的にウォールクロックテストと指定されない限り、固定/フェイクのクロックを使用すること。
- フィクスチャにはバージョン/ハッシュのメタデータを含めること。
- Nightly の失敗はリプレイ用の成果物（ログ、シード、フィクスチャバージョン）を保持すること。
- Manual+CI テストは PR に再現可能なランブックの証跡を含めること。

## 7. 実行環境の契約

- CI 環境:
  - tmux (`>= 3.3`) およびローカル sshd ハーネスを備えた Linux ランナー。
  - 本カタログの CI ラベル付きテストをすべて実行すること。
  - tmux のセッション名とソケットパスは、並列実行のためテストごとに一意であること。
- Nightly 環境:
  - セクション4のベンチマークワークロードを伴うマルチターゲットプロファイル（ホスト + SSH ターゲット）。
  - Nightly ラベル付きテストをすべて実行し、リプレイ用の成果物を保持すること。
  - 必須成果物: ログ、メトリクス、Property シード、フィクスチャのバージョン/ハッシュ。
- Manual+CI 環境:
  - 再現可能なランブックと証跡成果物を PR に添付すること。

## 8. 報告とトレーサビリティ

ランタイム/API/アクションの挙動に影響する PR には以下を含めること:

- `docs/tasks.md` からの参照タスクID。
- 本カタログからの参照テストID。
- ゲート影響ステートメント（`どのフェーズゲートに影響するか`）。
