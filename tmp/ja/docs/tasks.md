# AGTMUX タスク一覧 (v0.5)

Date: 2026-02-13
Status: Draft
Source Spec: `docs/agtmux-spec.md`
Plan Reference: `docs/implementation-plan.md`

## 1. タスクバックログ

| ID | フェーズ | 優先度 | タスク | FR/NFR | 依存先 | 受け入れ基準 | テストID | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| TASK-001 | 0 | P0 | すべてのコアテーブルに対する基本 SQLite マイグレーションの追加 | FR-3, FR-14, FR-15 | - | クリーンな DB に対してマイグレーションの適用・ロールバックが正常に動作する | TC-001, TC-002 | Todo |
| TASK-002 | 0 | P0 | アクティブランタイムの部分一意インデックスの適用 | FR-13 | TASK-001 | ペインごとに `ended_at IS NULL` のランタイムが1つだけ存在する | TC-003 | Todo |
| TASK-003 | 0 | P0 | ランタイム識別子ライフサイクルの実装（`tmux_server_boot_id`, `pane_epoch`） | FR-13 | TASK-001 | ランタイムのロールオーバーでエポックが更新され、古い参照が拒否される | TC-004, TC-005 | Todo |
| TASK-004 | 0 | P0 | 順序付け・重複排除コンパレータおよび取り込みべき等性パスの実装 | FR-14, NFR-4, NFR-8 | TASK-001 | シャッフルされた同一ストリームに対する決定的出力と、重複安全な適用 | TC-006, TC-007 | Todo |
| TASK-005 | 0 | P0 | `event_inbox` バインドリゾルバの実装 | FR-14 | TASK-001, TASK-003 | 保留中のバインドが安全な候補に対してのみ解決される | TC-008, TC-009 | Todo |
| TASK-006 | 0 | P1 | リコンサイラの stale/health 遷移の実装 | FR-8, NFR-2 | TASK-004 | ダウン/stale シグナルが安全に unknown に収束する | TC-010 | Todo |
| TASK-007 | 0 | P1 | ペイロード秘匿化とデータ保持ジョブの追加 | NFR-9 | TASK-001 | 秘匿化されていないペイロードが SQLite に永続化されない | TC-011, TC-012, TC-050 | Todo |
| TASK-008 | 0 | P1 | 仕様ベースラインからのインデックスセットの追加 | NFR-1, NFR-3 | TASK-001 | クエリプランが期待されるインデックスの使用を確認する | TC-013 | Todo |
| TASK-009 | 1 | P0 | ターゲットマネージャコマンドの実装 | FR-10 | TASK-001, TASK-031 | ローカルおよび SSH ターゲットに対して add/connect/list/remove が動作する | TC-014 | Todo |
| TASK-010 | 1 | P0 | Claude アダプタの実装 | FR-1, FR-2 | TASK-003, TASK-004 | Claude のシグナルが正規化された状態に変換される | TC-015 | Todo |
| TASK-011 | 1 | P0 | Codex アダプタの実装 | FR-1, FR-2 | TASK-003, TASK-004 | Codex の notify/wrapper イベントが正しく正規化される | TC-016 | Todo |
| TASK-012 | 1 | P0 | API v1 読み取りエンドポイントの実装 | FR-4, FR-6, FR-7, FR-9 | TASK-004, TASK-006 | panes/windows/sessions の JSON コントラクトがグルーピングカウントとマルチターゲット集約セマンティクスで安定している | TC-017 | Todo |
| TASK-013 | 1 | P0 | watch ストリームの実装（`stream_id`, `cursor`） | FR-4, FR-6 | TASK-012 | 再開セマンティクスがリスタート/有効期限テストに合格する | TC-019, TC-020, TC-051 | Todo |
| TASK-014 | 1 | P0 | CLI の list/watch を API v1 にマッピングする実装 | FR-4, FR-6 | TASK-012, TASK-013 | CLI 出力と JSON が API スキーマと一致する | TC-021 | Todo |
| TASK-015 | 1 | P0 | スナップショット検証付き attach アクションの実装 | FR-5, FR-15 | TASK-003, TASK-012, TASK-018 | 古いランタイム/スナップショットでの attach が拒否される | TC-022 | Todo |
| TASK-016 | 1 | P1 | 部分結果レスポンスエンベロープの実装 | NFR-7 | TASK-012 | `partial`, `target_errors`, requested/responded ターゲットが出力される | TC-023 | Todo |
| TASK-017 | 1 | P1 | CLI/API における `target-session` エンコーディングルールの正規化 | FR-16 | TASK-012 | エンコードされたセッション名が安全にラウンドトリップする | TC-024 | Todo |
| TASK-018 | 1 | P0 | べき等性を備えた共有 `actions` 書き込みパスの実装 | FR-15, NFR-4 | TASK-003, TASK-012 | 同一の request_ref が同一の action_id/result を返す | TC-025, TC-026 | Todo |
| TASK-019 | 1.5 | P0 | `text/stdin/key/paste` モードを備えた `send` の実装 | FR-5 | TASK-018 | モードの動作が仕様に一致し、ガードチェックが機能する | TC-027 | Todo |
| TASK-020 | 1.5 | P0 | `view-output` アクションの実装 | FR-5 | TASK-018 | 範囲制限付きキャプチャが正しいペイン出力を返す | TC-028 | Todo |
| TASK-021 | 1.5 | P0 | `kill` モード `key\|signal` およびガードロジックの実装 | FR-5, FR-15 | TASK-018 | signal モードで pid が存在しない場合に `E_PID_UNAVAILABLE` が返される | TC-029, TC-030 | Todo |
| TASK-022 | 1.5 | P1 | アクション-イベント監査相関の実装 | FR-15 | TASK-018 | action_id が関連イベントまで追跡可能である | TC-031 | Todo |
| TASK-023 | 1 | P1 | 構造化エラーエンベロープとコードマッピングの実装 | FR-16 | TASK-012, TASK-018 | 機械可読なエラーオブジェクトが API/CLI 間で安定している | TC-018, TC-032, TC-053 | Todo |
| TASK-024 | 2 | P0 | Gemini アダプタの実装 | FR-1, FR-2 | TASK-004, TASK-006 | Gemini のパーサー/ラッパーの遷移が収束する | TC-033 | Todo |
| TASK-025 | 2 | P1 | 再接続/バックオフの信頼性強化 | NFR-1, NFR-7 | TASK-024 | ターゲットの継続的なフラップに対してシステムが収束状態を維持する | TC-034, TC-052 | Todo |
| TASK-026 | 2 | P1 | v1 の JSON スキーマ互換性テスト | FR-6 | TASK-012, TASK-013, TASK-023 | スキーマ互換性テストスイートがリリースのゲートとなる | TC-035, TC-045 | Todo |
| TASK-027 | 2.5 | P1 | Copilot CLI アダプタの追加 | FR-12 | TASK-025, TASK-026, TASK-035 | アダプタが共通コントラクトテストに合格する | TC-036 | Todo |
| TASK-028 | 2.5 | P1 | Cursor CLI アダプタの追加 | FR-12 | TASK-025, TASK-026, TASK-035 | アダプタが共通コントラクトテストに合格する | TC-037 | Todo |
| TASK-029 | 3 | P1 | API v1 を使用した macOS アプリ読み取りビューの構築 | Goal | TASK-012, TASK-013 | アプリがグローバル/セッション/ウィンドウ/ペインビューを描画できる | TC-038 | Todo |
| TASK-030 | 3 | P1 | 同等の安全チェックを備えた macOS アプリアクションの構築 | Goal, FR-15 | TASK-018, TASK-019, TASK-021 | アプリのアクションがフェイルクローズドセマンティクスを維持する | TC-039 | Todo |
| TASK-031 | 0 | P0 | `TargetExecutor` とデーモン境界の実装 | FR-9 | TASK-001 | すべてのターゲット読み書きパスが UDS トランスポートとヘルスエンドポイントコントラクトを備えた executor 抽象化を経由する | TC-040 | Todo |
| TASK-032 | 0 | P0 | ターゲットごとの tmux トポロジオブザーバの実装 | FR-3, FR-9 | TASK-031 | トポロジスナップショットがターゲット間で収束する | TC-041 | Todo |
| TASK-033 | 1 | P1 | グルーピングとサマリーロールアップの実装 | FR-7 | TASK-012 | セッション/ウィンドウサマリーが仕様セクション 7.6 に一致する | TC-042 | Todo |
| TASK-034 | 1 | P1 | マルチターゲット集約レスポンスセマンティクスの適用 | FR-9, NFR-7 | TASK-009, TASK-012, TASK-016 | requested/responded/target_errors の整合性 | TC-043 | Todo |
| TASK-035 | 2 | P0 | アダプタレジストリのケーパビリティ駆動ディスパッチの実装 | FR-11, NFR-5 | TASK-024 | コアエンジンの変更なしにアダプタを追加できる | TC-046 | Todo |
| TASK-036 | 2 | P1 | アダプタコントラクトバージョン互換性チェックの追加 | NFR-6 | TASK-035 | 後方互換のマイナーバージョン変更が検証される | TC-047 | Todo |
| TASK-037 | 2 | P1 | より高度な list/watch フィルタとソートの追加 | FR-4, FR-7 | TASK-012, TASK-013 | フィルタ/ソートのコントラクトが安定かつ決定的である | TC-048 | Todo |
| TASK-038 | 0 | P1 | 取り込み時の重複ストーム動作の堅牢化 | NFR-4, FR-14 | TASK-004, TASK-005 | 重複/リトライストームが状態の不整合を生まない | TC-049 | Todo |
| TASK-039 | 1 | P1 | 可視化レイテンシベンチマークハーネスと SLO ゲートの追加 | NFR-1 | TASK-012, TASK-013, TASK-031 | ベンチマークプロファイルが再現可能であり、p95 可視化遅延 <= 2秒を強制する | TC-044 | Todo |
| TASK-040 | 0 | P0 | CI/Nightly 実行ベースラインの確立（tmux + ローカル sshd） | NFR-1, NFR-7 | TASK-001, TASK-031 | CI が tmux/ターゲット統合テストスイートを実行し、Nightly がマルチターゲット/信頼性スイートをアーティファクト付きで実行する | TC-040, TC-041, TC-044, TC-052 | Todo |

## 2. 直近スプリント候補（推奨）

- TASK-001
- TASK-002
- TASK-003
- TASK-004
- TASK-005
- TASK-006
- TASK-031
- TASK-032
- TASK-040

## 3. 完了の定義

タスクが完了とみなされるのは、以下のすべてを満たした場合のみです：

- 受け入れ基準を満たしている。
- 関連する CI テストが自動化され、パスしている。
- 関連する Nightly テストが最新のスケジュール実行でグリーンである。
- 手動 + CI テストの再現可能な実行エビデンスが PR に含まれている。
- API/コントラクトの変更が仕様書およびテストカタログに反映されている。
- 動作変更時の運用ログ/エラー表示がドキュメント化されている。
