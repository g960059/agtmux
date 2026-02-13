# AGTMUX Spec (v0.5)

Date: 2026-02-13
Status: Draft

## 1. 背景

現在のワークフローでは、セッション/ウィンドウ/ペインごとの作業整理に tmux を利用しており、エージェント CLI（Claude、Codex、Gemini）はペイン内で実行されている。
現状の課題は、オペレーターの状況把握が手動ポーリングに依存していることである。エージェントが実行中か、ユーザー入力待ちか、完了済みか、アイドル状態かを知るために、各ペインを繰り返し確認する必要がある。

目指す方向性：
- tmux + エージェント状態に対する tcmux ライクな可視性
- マルチエージェント対応（Claude、Codex、Gemini）
- 将来のエージェント向けアダプターベースの拡張性（例：Copilot CLI、Cursor CLI）
- tmux セッション/ウィンドウごとのグルーピングとサマリー
- ホストと VM をまたいだ統合ビュー（host、vm1、vm2）
- 将来的な macOS 常駐アプリによる高速な操作

## 2. 課題

- ペインをまたいだ常時オンの統合ステータスビューが存在しない。
- 手動ポーリングはコストが高く、見落としが発生しやすい。
- エージェントの状態セマンティクスは CLI ごとに異なり、正規化されていない。
- セッション/ウィンドウ/ペインが増えると tmux の構造がノイジーになる。
- `send`、`attach`、`view output`、`kill` を統一的に操作するコントロールサーフェスが存在しない。
- ホスト/VM のコンテキストが運用上分離されており、グローバルな状況把握が低下している。

## 3. ゴール

- 手動ポーリングなしで、エージェントペインの常時利用可能な状態可視性を提供する。
- Claude、Codex、Gemini を統一された状態モデルでサポートする。
- tcmux ライクなリストコマンドを提供する：
  - エージェントペインの一覧
  - ウィンドウの一覧（エージェントステータスサマリー付き）
  - エージェントセッションの一覧
- ターゲットの同一性を維持しつつ、ホストと VM のターゲットを一つのビューに統合する。
- ステータスで分類する（running、waiting_input、waiting_approval、completed、idle、error、unknown）。
- プロジェクトレベルの素早い状況把握のために、tmux セッションごとにクロスターゲットで分類する。
- クロスターゲットでのセッション名衝突を避けるため、デフォルトのグルーピングは `target-session` とする。
- 一つのツーリングサーフェスから運用操作を可能にする：
  - send
  - attach
  - view output
  - kill
- 同じバックエンドが CLI と将来の macOS 常駐アプリの両方を駆動できるよう、再利用可能なアーキテクチャを維持する。

## 4. 非ゴール（初期）

- tmux 自体の置き換え。
- 完全なターミナルエミュレーター UI の構築。
- あらゆるサードパーティ CLI の出力形式に対する完全なセマンティック理解。
- v0 におけるクラウド同期やマルチホスト分散オーケストレーション。

## 5. ユーザーストーリー

1. 開発者として、ホストと VM をまたいで全エージェントペインとその状態を一度に確認したい。各ペインを手動で確認する作業をなくすためである。
2. 開発者として、ターゲットをまたいだセッションレベルのサマリーをグルーピングして見たい。どのプロジェクト領域に注意が必要かを素早く判断するためである。
3. 開発者として、状態でフィルタリングしたい（例：waiting_input/waiting_approval）。介入の優先順位を決めるためである。
4. 開発者として、エージェントが待機しているペインに正確にアタッチしたい。
5. 開発者として、コンテキストを切り替えずに CLI/アプリからペインに入力を送信したい。
6. 開発者として、最近の出力を確認してから、エージェントを安全にキルまたは再開したい。

## 6. 要件

### 6.1 機能要件

- FR-1: MVP では Claude と Codex、Phase 2 までに Gemini のエージェント状態を検知・永続化する。
- FR-2: エージェント固有のシグナルを共通の状態モデルに正規化する。
- FR-3: tmux ペインとエージェントランタイムメタデータ間のマッピングを維持する。
- FR-4: 状態サマリー付きのペイン/ウィンドウ/セッション一覧を提供する。
- FR-5: コントロールコマンドをサポートする：send、attach、view-output、kill。
- FR-6: 全リストコマンドで機械可読出力（`--json`）を提供する。
- FR-7: セッション/ウィンドウのグルーピングと状態別カウントを含む。
- FR-8: 古い状態が自己修復するリコンシリエーション機構を含む。
- FR-9: 複数のターゲット（host/vm1/vm2）を一つの集約ビューでサポートする。
- FR-10: ターゲットコマンド（`add`、`connect`、`list`、`remove`）とターゲットスコープの操作を提供する。
- FR-11: コアの状態エンジンを変更せずに新しいエージェント CLI を追加できるアダプターレジストリを提供する。
- FR-12: Copilot CLI や Cursor CLI などの将来のアダプターをサポートする。
- FR-13: 古いアクション/イベント適用を防ぐランタイム同一性ガード（`runtime_id`/`pane_epoch`）を導入する。
- FR-14: 決定的なイベント重複排除および順序付けセマンティクスを定義する。
- FR-15: 全アクションはサーバーサイドのスナップショット検証によるフェイルクローズド前提条件を強制する。
- FR-16: 正規のアクション参照文法と曖昧性のない解決ルールを定義する。

### 6.2 非機能要件

- NFR-1: ほぼリアルタイムの更新（目標：可視ラグ <= 2 seconds）。
- NFR-2: 安全な障害モード：不正確な確定状態ではなく unknown 状態とする。
- NFR-3: ホストの CPU/メモリへの低オーバーヘッド。
- NFR-4: べき等なイベントハンドリングおよび重複イベントへの堅牢性。
- NFR-5: 将来のエージェント向けの拡張可能なアダプターモデル。
- NFR-6: アダプター契約の安定性（マイナーバージョン間で後方互換性のあるインターフェース）。
- NFR-7: ターゲット障害時の部分結果動作（グローバルリスティングをブロックしない）。
- NFR-8: 同一入力イベントストリームに対する決定的な収束。
- NFR-9: 機密接続データおよび未編集ペイロードをプレーンテキストの状態 DB に保存してはならない。

## 7. 仕様

### 7.1 コアアーキテクチャ

- Target Manager:
  - 既知のターゲット（`host`、`vm1`、`vm2`、...）を保存する。
  - 接続性とリモートコマンド実行コンテキストを管理する。
- Target Executor:
  - tmux 操作およびアダプターランタイム向けの、ローカル/SSH 統一実行抽象化。
  - ターゲットへの全読み取り/書き込み操作に必須。
- Collector（ターゲットごと）:
  - 一つのターゲットから tmux トポロジーとアダプターシグナルを収集する。
- Aggregator / Daemon（`agtmuxd`）:
  - コレクターからのイベントをマージする。
  - 状態エンジンと永続化を所有する。
  - CLI および将来の macOS アプリ向けの読み取り/書き込み API を提供する。
- Agent Adapter Registry:
  - `agent_type` をアダプター実装とケイパビリティにマッピングする。
  - コアエンジンはアダプターインターフェースにのみ依存し、具体的なエージェントロジックには依存しない。
- Agent Adapters（初期）:
  - Claude アダプター：フック駆動イベント。
  - Codex アダプター：通知駆動イベント＋ラッパーライフサイクルシグナル。
  - Gemini アダプター：ラッパーライフサイクル＋出力パーサーシグナル。
  - 将来のアダプター：Copilot CLI、Cursor CLI（コアの再設計不要）。
- Tmux Observer:
  - tmux メタデータを通じてターゲットごとのペイン/セッション/ウィンドウトポロジーを追跡する。
- State Engine:
  - アダプターイベントと tmux 観測結果を正規状態にマージする。
- State Store:
  - 信頼できる唯一の情報源としての永続ストア（SQLite 推奨）。
- Presentation Layer:
  - 現在は CLI コマンド。
  - 将来は macOS 常駐アプリ（同じデーモン API を参照）。

### 7.1.1 状態所有権の境界

- `states` テーブルは State Engine のみが所有する。
- Reconciler は `states` の行を直接書き込んではならない。
- Reconciler は合成リコンシリエーションイベント（`stale_detected`、`target_health_changed`、`demotion_due`）を同じ取り込みパスに発行する。
- State Engine はアダプターイベントとリコンシラーイベントの両方に同じ重複排除/順序付け/ランタイムガードを適用する。

### 7.1.2 TargetExecutor と SSH ライフサイクル

- TargetExecutor は `local` と `ssh` ターゲットに対して同一の読み取り/書き込み契約を公開しなければならない。
- SSH モードはコマンドごとのハンドシェイクオーバーヘッドを回避するため、永続的な接続再利用（ControlMaster 相当/セッションプール）を使用しなければならない。
- デフォルト実行タイムアウト（設定可能）：
  - 接続タイムアウト：3 seconds
  - コマンドタイムアウト：5 seconds
  - リトライポリシー：指数バックオフ（250ms、1s）とジッター付きの 2 回リトライ
- デフォルトターゲットヘルス遷移ポリシー（設定可能）：
  - `ok -> degraded`：最初のプローブ/コマンド失敗
  - `degraded -> down`：30 seconds 以内に 3 回連続失敗
  - `degraded|down -> ok`：2 回連続の成功プローブ＋トポロジー取得の成功
- ターゲットが `down` の場合、集約読み取りは部分結果とターゲットスコープの `E_TARGET_UNREACHABLE` で継続しなければならない。

### 7.1.3 ランタイム前提条件

- サポートする tmux バージョンは `>= 3.3`。
- `tmux_server_boot_id` は一つの tmux サーバーのライフタイムにおいて安定でなければならない。
- ターゲット環境で直接 boot-id が利用できない場合、実装はサーバーメタデータ（例：サーバー pid + サーバー起動タイムスタンプ）から安定した同等値を導出しなければならない。

### 7.2 正規状態モデル

正規状態：
- `running`
- `waiting_input`
- `waiting_approval`
- `completed`
- `idle`
- `error`
- `unknown`

状態優先度（高い順）：
1. `error`
2. `waiting_approval`
3. `waiting_input`
4. `running`
5. `completed`
6. `idle`
7. `unknown`

デフォルト値（推奨）：
- `completed` は 120 seconds 後に自動的に `idle` に降格する（設定可能）。
- `kill` のデフォルトシグナルは `INT`。

注記：
- `completed` は最後のタスクが終了し、オペレーターの認識のためにまだ新鮮な状態を意味する。
- 解決順序は `dedupe/order check -> runtime guard -> freshness check -> precedence`。
- 不明または古い情報を確信度の高いアクティブ状態に昇格させてはならない。

### 7.2.1 遷移と安全性ルール

- すべてのイベントは安全に重複排除および順序付けするための十分なメタデータを含まなければならない。
- ランタイム同一性が現在のペインランタイムと一致しない場合、イベント適用は破棄されなければならない。
- 優先度は新鮮な候補シグナル間でのみ適用される。
- 降格ジョブ（`completed -> idle`）はバージョン/ランタイムガードを含まなければならない。
- 降格の時間基準はデーモンの `ingested_at` である（リモートの壁時計ではない）。
- `unknown` は `reason_code` を含まなければならない（例：`stale_signal`、`target_unreachable`、`unsupported_signal`）。
- ターゲットヘルスが `down` またはシグナルが TTL を超えて古い場合、状態解決は `unknown` にショートサーキットしなければならない。

### 7.2.2 順序付けと重複排除アルゴリズム

- イベント適用順序は決定的である：
  1. `(runtime_id, source, dedupe_key)` による重複を拒否する
  2. 順序付けキーを比較する：
     - `source_seq` が利用可能な場合（同一 `runtime_id + source`）
     - それ以外は `effective_event_time` を使用、ここで
       - `effective_event_time = event_time`（`abs(event_time - ingested_at) <= skew_budget` の場合）
       - `effective_event_time = ingested_at`（スキューがバジェットより大きい場合）
       - デフォルトの `skew_budget` は 10 seconds（設定可能）
     - 次に `ingested_at`
     - 次に `event_id`
  3. キーが当該 `runtime_id + source` の保存済みカーソルより新しい場合のみ適用する
- クロスソースのスタベーションを避けるため、ソース固有のカーソルを維持しなければならない。

### 7.2.3 アダプター契約

各アダプターは共通の契約を実装しなければならない：
- `ContractVersion() -> string`
- `IdentifyProcess(ctx, pane) -> match_result`
- `Subscribe(ctx, pane) -> signal_stream`（イベント駆動アダプター用）
- `Poll(ctx, pane) -> []signal`（ポーリングフォールバック用）
- `Normalize(signal) -> state_transition`
- `Health(ctx) -> status`

ケイパビリティはアダプターが宣言する：
- `event_driven`
- `polling_required`
- `supports_waiting_approval`
- `supports_waiting_input`
- `supports_completed`

コア動作：
- State Engine は正規化された遷移のみを消費する。
- 不明またはサポートされていないシグナルは `unknown` に降格しなければならず、状態を捏造してはならない。

### 7.2.4 ランタイム同一性とペインエポックルール

- ランタイム同一性はペインインスタンスに紐づく：
  - `pane_instance = (target_id, tmux_server_boot_id, pane_id)`
- `pane_epoch` は以下のいずれかが発生した場合にインクリメントしなければならない：
  - レイアウト変更やリスタート後に同じ `pane_id` でペインが再作成された場合
  - アダプター/オブザーバーがペインのランタイムプロセス同一性（`pid`）の変化を検出した場合
  - オブザーバーの再同期で、現在のペインプロセス同一性と一致しないアクティブランタイム行が見つかった場合
- `runtime_id` はランタイムメタデータから一意かつ再現可能でなければならない：
  - 推奨される導出方法：
    - `sha256(target_id + tmux_server_boot_id + pane_id + pane_epoch + agent_type + started_at_ns)`
- `(target_id, pane_id)` ごとにアクティブなランタイム（`ended_at IS NULL`）は最大一つまで許可される。
- 古いランタイム同一性を参照するイベントまたはアクションは拒否されなければならない（`E_RUNTIME_STALE` / 前提条件失敗）。

### 7.3 データモデル（SQLite ドラフト）

- `targets`
  - `target_id` (PK)
  - `target_name`（`host`、`vm1`、`vm2`、...）
  - `kind`（`local`/`ssh`）
  - `connection_ref`（非機密参照、例：SSH ホストエイリアス）
  - `is_default`
  - `last_seen_at`
  - `health`（`ok`/`degraded`/`down`）
  - `updated_at`
- `panes`
  - `target_id`
  - `pane_id`
  - `session_name`
  - `window_id`
  - `window_name`
  - `updated_at`
  - PK: (`target_id`, `pane_id`)
- `runtimes`
  - `runtime_id` (PK)
  - `target_id`
  - `pane_id`
  - `tmux_server_boot_id`
  - `pane_epoch`
  - `agent_type`
  - `pid`（nullable）
  - `started_at`
  - `ended_at`（nullable）
  - UNIQUE: (`target_id`, `tmux_server_boot_id`, `pane_id`, `pane_epoch`)
  - アクティブランタイム不変条件：
    - (`target_id`, `pane_id`) ごとにアクティブなランタイム（`ended_at IS NULL`）は最大一つ
    - DB レベルで部分ユニークインデックスにより強制
- `events`
  - `event_id` (PK)
  - `runtime_id`
  - `event_type`
  - `source`（`hook`/`notify`/`wrapper`/`poller`）
  - `source_event_id`（nullable）
  - `source_seq`（nullable）
  - `event_time`
  - `ingested_at`
  - `dedupe_key` (NOT NULL)
  - `action_id`（nullable、FK -> `actions.action_id`）
  - `raw_payload`（編集済み形式；ポリシーによりオプション）
  - UNIQUE: (`runtime_id`, `source`, `dedupe_key`)
- `event_inbox`
  - `inbox_id` (PK)
  - `target_id`
  - `pane_id`
  - `runtime_id`（nullable）
  - `event_type`
  - `source`
  - `dedupe_key`
  - `event_time`
  - `ingested_at`
  - `pid`（nullable）
  - `start_hint`（nullable）
  - `status`（`pending_bind`/`bound`/`dropped_unbound`）
  - `reason_code`（nullable）
  - `raw_payload`（編集済み形式；ポリシーによりオプション）
  - UNIQUE: (`target_id`, `pane_id`, `source`, `dedupe_key`)
- `runtime_source_cursors`
  - `runtime_id`
  - `source`
  - `last_source_seq`（nullable）
  - `last_order_event_time`
  - `last_order_ingested_at`
  - `last_order_event_id`
  - PK: (`runtime_id`, `source`)
- `states`
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `state`
  - `reason_code`
  - `confidence`（`high`/`medium`/`low`）
  - `state_version`
  - `last_source_seq`（nullable）
  - `last_seen_at`
  - `updated_at`
  - PK: (`target_id`, `pane_id`)
- `action_snapshots`
  - `snapshot_id` (PK)
  - `action_id`（FK -> `actions.action_id`）
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `state_version`
  - `observed_at`
  - `expires_at`
  - `nonce`
- `actions`
  - `action_id` (PK)
  - `action_type`（`attach`/`send`/`view-output`/`kill`）
  - `request_ref`（必須のべき等キー；UUIDv7 または ULID 推奨）
  - `target_id`
  - `pane_id`
  - `runtime_id`
  - `requested_at`
  - `completed_at`（nullable）
  - `result_code`
  - `error_code`（nullable）
  - `metadata_json`
  - UNIQUE: (`action_type`, `request_ref`)
- `adapters`
  - `adapter_name` (PK)
  - `agent_type`
  - `version`
  - `capabilities` (JSON)
  - `enabled`
  - `updated_at`

### 7.3.1 データ保護、保持、およびインデックスベースライン

- `targets.connection_ref` は非機密参照のみを保存しなければならない（例：SSH ホストエイリアス）。
- `events.raw_payload` はデフォルトで編集済みコンテンツとして保存しなければならない。
- 未編集ペイロードはいかなるモードでも SQLite に永続化してはならない。
- デバッグモードでは、未編集ペイロードをメモリ内または最大 TTL 24 hours の暗号化一時ファイルストレージにのみ保持できる。
- デフォルト保持ポリシー：
  - `events` 生ペイロード：7 days
  - `events` メタデータ行：14 days（設定可能）
- 必須ベースラインインデックス：
  - ユニーク部分：`runtimes(target_id, pane_id) WHERE ended_at IS NULL`
  - ユニーク：`actions(action_type, request_ref)`
  - `events(runtime_id, source, ingested_at DESC)`
  - `events(ingested_at DESC)`
  - `event_inbox(status, ingested_at)`
  - `states(updated_at DESC)`
  - `states(state, updated_at DESC)`

### 7.4 シグナル取り込み

### 7.4.1 Event Envelope v1（規範的）

共通イベントエンベロープフィールド：
- `event_id`（必須、string/UUID）
- `event_type`（必須、string）
- `source`（必須、enum: `hook|notify|wrapper|poller`）
- `dedupe_key`（必須、string、非空）
- `source_event_id`（オプション、string）
- `source_seq`（オプション、int64）
- `event_time`（必須、ソースからのタイムスタンプ）
- `ingested_at`（必須、デーモンタイムスタンプ）
- `runtime_id`（バインド済みイベントでは必須、バインド保留イベントではオプション）
- `target_id`（`runtime_id` が不在の場合に必須）
- `pane_id`（`runtime_id` が不在の場合に必須）
- `pid`（オプション、バインド保留ヒント）
- `start_hint`（オプション、バインド保留ヒントタイムスタンプ）
- `raw_payload`（オプション；デフォルトポリシーでは編集済み形式）

エンベロープルール：
- `dedupe_key` は `source_event_id` が存在する場合でも常に存在しなければならない。
- 推奨される `dedupe_key` 導出方法：
  - `sha256(source + ":" + coalesce(source_event_id,"") + ":" + normalize(payload_hash) + ":" + normalize(event_type))`
- `source_seq` が不在の場合、順序付けは 7.2.2 の `effective_event_time` に依存する。
- `ingested_at` は鮮度および降格タイミングの判断における権威あるタイムスタンプである。

ランタイムバインディングルール：
- アダプターイベントに `runtime_id` がない場合、`target_id + pane_id（+ pid/start_hint がある場合）` で `pending_bind` として取り込む。
- Resolver は利用可能な全ヒントを満たすアクティブランタイム候補が正確に一つ存在する場合にのみバインドしなければならない。
  - 候補セットは `target_id + pane_id` でフィルタリングされる
  - `pid` が存在する場合、候補ランタイムの `pid` が一致しなければならない
  - `start_hint` が存在する場合、`abs(runtime.started_at - start_hint)` がバインドウィンドウ内（デフォルト 5 seconds）でなければならない
- Resolver は以下の場合に保留イベントを `dropped_unbound` として破棄しなければならない：
  - 候補が存在しない場合（`reason_code = bind_no_candidate`）
  - 候補が複数存在する場合（`reason_code = bind_ambiguous`）
  - 安全な解決前にバインド保留 TTL が期限切れになった場合（`reason_code = bind_ttl_expired`）
- `bound` イベントのみが状態遷移の対象となる。

アダプター固有の取り込み：
- Claude:
  - ライフサイクルおよびインタラクション要求シグナルにフックを使用する。
- Codex:
  - `notify` イベント（`approval-requested`、`agent-turn-complete`）＋ラッパー開始/終了シグナルを使用する。
- Gemini:
  - ラッパー開始/終了シグナル＋設定可能なパーサーパターンを使用する。
- Copilot CLI（将来）:
  - アダプターはラッパー＋パーサーアプローチで開始し、ネイティブイベントが公開された場合はそちらに移行する。
- Cursor CLI（将来）:
  - アダプターはラッパー＋パーサーアプローチで開始し、ネイティブイベントが公開された場合はそちらに移行する。

Reconciler:
- 定期的な tmux スキャン（アクティブペインではデフォルト 2 seconds）。
- アイドルペインでは指数バックオフ。
- Reconciler はリコンシリエーションイベントを発行し、`states` を直接変更してはならない。
- 古い状態の降格とターゲットヘルス遷移は Reconciler が所有する判断であり、State Engine が適用する。
- ターゲット到達不能遷移は低信頼度で `unknown/target_unreachable` を発行しなければならない。

### 7.5 CLI サーフェス（MVP）

ターゲット管理：
- `agtmux target add <name> --kind local|ssh [--ssh-target <ssh_host>]`
- `agtmux target connect <name>`
- `agtmux target list [--json]`
- `agtmux target remove <name> [--yes]`

一覧表示とアクション（デフォルトで集約）：
- `agtmux list panes [--target <name>|--all-targets] [--target-session <target>/<session-enc>] [--session <name>] [--state <state>] [--agent <type>] [--needs-action] [--json]`
- `agtmux list windows [--target <name>|--all-targets] [--target-session <target>/<session-enc>] [--session <name>] [--with-agent-status] [--json]`
- `agtmux list sessions [--target <name>|--all-targets] [--agent-summary] [--group-by target-session|session-name] [--json]`
- `agtmux attach <ref> [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux send <ref> (--text <text>|--stdin|--key <key>) [--enter] [--paste] [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale]`
- `agtmux view-output <ref> [--lines <n>]`
- `agtmux kill <ref> [--mode key|signal] [--signal INT|TERM|KILL] [--if-runtime <runtime_id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale] [--yes]`
- `agtmux watch [--target <name>|--all-targets] [--scope panes|windows|sessions] [--format table|jsonl] [--interval <duration>] [--cursor <stream_id:sequence>] [--once]`

デフォルトオプション値：
- `watch --interval`: `2s`
- `view-output --lines`: `200`

正規アクション参照文法（BNF）：

```txt
<ref> ::= <runtime-ref> | <pane-ref>
<runtime-ref> ::= "runtime:" <runtime-id>
<runtime-id> ::= /[A-Za-z0-9._:-]{16,128}/
<pane-ref> ::= "pane:" <target> "/" <session-enc> "/" <window-id> "/" <pane-id>
<target> ::= /[A-Za-z0-9._-]+/
<session-enc> ::= RFC3986 percent-encoded session name
<window-id> ::= "@" <digits>
<pane-id> ::= "%" <digits>
```

参照コンポーネントルール：
- `target` は登録済みのターゲット名である。
- `session-enc` はパース前にパーセントエンコードされていなければならない。
- `window-id` / `pane-id` は表示名ではなく、tmux の不変 ID を使用しなければならない。
- `--target-session` は同じエンコーディングルールに従い `<target>/<session-enc>` を使用しなければならない。

参照解決：
1. パース失敗 -> `E_REF_INVALID`
2. デコード失敗 -> `E_REF_INVALID_ENCODING`
3. 一致なし -> `E_REF_NOT_FOUND`
4. 複数一致 -> `E_REF_AMBIGUOUS`（アクションを実行してはならない）
5. 単一一致 -> 実行前にサーバーサイドでアクションスナップショットが作成される

出力仕様：
- デフォルト：人間が読みやすいテーブル。
- `--json`：自動化および将来の UI 向けの安定スキーマ。
- `watch --format jsonl` はイベントごとに安定スキーマで 1 行出力しなければならない。
- JSON は `schema_version`、`generated_at`、`filters`、`summary`、および各アイテムの `identity` を含まなければならない。
- 集約読み取り中にいずれかのターゲットが失敗した場合、JSON は以下を含まなければならない：
  - `partial`（boolean）
  - `requested_targets`
  - `responded_targets`
  - `target_errors`（ターゲットごとのエラーリスト）
- スコープ別の同一性フィールド：
  - `panes`: `target`、`session_name`、`window_id`、`pane_id`
  - `windows`: `target`、`session_name`、`window_id`
  - `sessions`: `target`、`session_name`

### 7.5.1 Watch JSONL 契約

- `watch --format jsonl` は UTF-8 JSON 行を出力し、イベントごとに 1 行とする。
- 行エンベロープフィールド：
  - `schema_version`
  - `generated_at`
  - `emitted_at`
  - `stream_id`
  - `cursor`（`<stream_id>:<sequence>`）
  - `scope`（`panes|windows|sessions`）
  - `type`（`snapshot|delta|reset`）
  - `sequence`（同一 `stream_id` 内で単調増加）
  - `filters`
  - `summary`
  - `items`（`snapshot` 用）
  - `changes`（`delta` 用）
- `items[].identity` はセクション 7.5 のスコープ固有の同一性要件に従わなければならない。
- `delta` 行は同一性付きのアイテムごとの操作（`upsert|delete`）を含まなければならない。
- `cursor` は同一ストリーム内で要求されたカーソルより大きいシーケンスを持つ最初のイベントから再開する。
- 提供されたカーソルが無効な場合：`E_CURSOR_INVALID`。
- カーソルが期限切れの場合（保持期間外）：`reset` を発行し、次に `snapshot` を発行する。API レスポンスモードでは `E_CURSOR_EXPIRED` を返す。

### 7.6 グルーピングとサマリー

- グローバル状態ロールアップ（デフォルトで全ターゲット）：
  - 状態別カウント
  - エージェントタイプ別カウント
  - ターゲット別カウント
- セッションレベルロールアップ：
  - デフォルトグルーピング：`target-session`
  - クロスターゲットでの `session-name` マージは `--group-by session-name` で明示的に指定
  - 各行にターゲットごとの内訳と合計カウントを含む
- ウィンドウレベルロールアップ：
  - 優先度による最上位状態
  - 待機カウント
  - 実行中カウント

### 7.7 コントロール動作

サーバーサイドアクションスナップショット：
- いかなるアクション（`attach`、`send`、`view-output`、`kill`）の前にも、デーモンは `actions` 行と `target`、`pane`、`runtime_id`、`state_version`、`observed_at`、`expires_at`、`nonce` を含む `action_snapshot` を作成しなければならない。
- デフォルトのスナップショット TTL は `30s`（`expires_at = observed_at + 30s`）であり、ポリシーにより上書きされない限りこの値を使用する。
- 現在の状態がスナップショットと一致しなくなり、`--force-stale` が明示的に設定されていない場合、アクション実行はフェイルクローズドでなければならない。
- CLI ガードフラグ（`--if-runtime`、`--if-state`、`--if-updated-within`）は追加の制約にすぎず、サーバーサイドの検証を弱めてはならない。
- アクション実行は期限切れのスナップショットを `E_SNAPSHOT_EXPIRED` で拒否しなければならない。
- アクション書き込み API は (`action_type`, `request_ref`) によりべき等でなければならない：
  - 同一キーの再実行は既存の `action_id` と保存済み結果を返す
  - 異なるペイロードでの競合する再実行は `E_IDEMPOTENCY_CONFLICT` を返す

- `send`:
  - ターゲット上の tmux を介してターゲットペインにキーストローク/テキストを送信する。
  - `--text` はリテラルテキストを送信する（シェル展開なし）。
  - `--stdin` は標準入力ペイロードを送信する（マルチライン入力用）。
  - `--key` は tmux キートークンを送信する（例：`C-c`、`Escape`）。
  - `--enter` はペイロードの後に Enter を追加する。
  - `--paste` はマルチラインの安全性のためにペーストバッファスタイルの配信を使用する。
  - TargetExecutor はローカルおよび SSH ターゲットの両方で、argv セーフな呼び出し（`no sh -c`）により tmux コマンドを実行しなければならない。
  - ランタイム/状態/鮮度ガードが一致しない場合、フェイルクローズドでなければならない。
- `attach`:
  - ユーザーをターゲット上のペイン/セッションに安全にジャンプさせる。
  - `--force-stale` が指定されない限り、アタッチ前に鮮度/ランタイムを検証しなければならない。
- `view-output`:
  - 行数制限付きのペインキャプチャを使用する。
- `kill`:
  - デフォルトモードは `key`；`INT` は `C-c` send-key 動作にマッピングされる。
  - `--mode signal` はターゲット上のランタイム `pid` に OS シグナルを送信する。
  - `TERM`/`KILL` は `--mode signal` でのみ有効。
  - `--mode signal` はランタイム `pid` が不明な場合、`E_PID_UNAVAILABLE` で失敗しなければならない。
  - デフォルトで確認プロンプトが必要；`--yes` で確認をスキップする。
  - ガード不一致（`if-runtime`、`if-state`、鮮度ウィンドウ）の場合、フェイルクローズドでなければならない。

### 7.8 macOS 常駐アプリ（将来）

- CLI と同じデーモン API を参照する。
- 主要画面：
  - グローバルサマリー（状態＋ターゲット）
  - セッションサマリー（クロスターゲット）
  - 選択したセッション内のウィンドウ
  - 状態と最終更新経過時間付きのペイン詳細リスト
- UI からのアクション：
  - send
  - attach
  - view output
  - kill

### 7.9 agtmuxd API v1（規範的最小仕様）

トランスポートとバージョニング：
- Daemon API v1 は CLI と macOS アプリの共有バックエンド契約である。
- デフォルトトランスポートは Unix ドメインソケット上の `HTTP/1.1 + JSON`。
- デフォルトソケットパスは `$XDG_RUNTIME_DIR/agtmux/agtmuxd.sock`（フォールバック：`~/.local/state/agtmux/agtmuxd.sock`）。
- ソケットパーミッションは `0600` でなければならない。
- オプションの TCP リスナーはループバック専用でデフォルトは無効；有効化時はトークンベースの認証が必要。
- レスポンスは `schema_version` を含まなければならない。
- 読み取りレスポンスは `generated_at` を含まなければならない。
- アクションレスポンスは `action_id`、`result_code`、および `completed_at`（完了時）を含まなければならない。

読み取りエンドポイント（最小）：
- `GET /v1/health`
- `GET /v1/targets`
- `GET /v1/panes`
- `GET /v1/windows`
- `GET /v1/sessions`
- `GET /v1/watch?scope=<panes|windows|sessions>&cursor=<stream_id:sequence>`

書き込みエンドポイント（最小）：
- `POST /v1/targets`
- `POST /v1/targets/{name}/connect`
- `DELETE /v1/targets/{name}`
- `POST /v1/actions/attach`
- `POST /v1/actions/send`
- `POST /v1/actions/view-output`
- `POST /v1/actions/kill`

デーモンライフサイクル（規範的最小仕様）：
- デーモンはユーザー/ワークスペーススコープごとのシングルインスタンスロックをサポートしなければならない。
- グレースフルシャットダウンは新規リクエストの受付を停止し、処理中の DB トランザクションをフラッシュし、ターミナル `reset` イベントで watch ストリームをクローズしなければならない。
- デーモン再起動後、watch ストリームは新しい `stream_id` を使用しなければならない；古いストリームからの古いカーソルはリセット動作に従わなければならない。

アクションリクエスト最小フィールド：
- `request_ref`（必須のべき等キー）
- `ref`
- `if_runtime`（オプション）
- `if_state`（オプション）
- `if_updated_within`（オプション）
- `force_stale`（オプション；デフォルト false）

エラーコード契約（最小）：
- `E_REF_INVALID`
- `E_REF_INVALID_ENCODING`
- `E_REF_NOT_FOUND`
- `E_REF_AMBIGUOUS`
- `E_RUNTIME_STALE`
- `E_PRECONDITION_FAILED`
- `E_SNAPSHOT_EXPIRED`
- `E_IDEMPOTENCY_CONFLICT`
- `E_CURSOR_INVALID`
- `E_CURSOR_EXPIRED`
- `E_PID_UNAVAILABLE`
- `E_TARGET_UNREACHABLE`

## 8. ハイレベルプラン / フェーズ

### 8.1 デリバリー成果物（推奨）

- アクティブフェーズごとにローリング実装計画ドキュメント（`plan`）を維持する。
- FR/NFR ID にリンクされた実行可能なタスク分解（`task`）を維持する。
- 各重要契約を自動化カバレッジにマッピングするテストカタログ（`test`）を維持する。
- 成果物がコード/仕様変更と共に更新されていることをフェーズ完了のゲートとする。

### Phase 0: コアランタイム

- 正規状態モデル、イベントエンベロープ、および遷移安全性ルールを定義する。
- `TargetExecutor` とデーモン境界（`agtmuxd`）を実装する。
- SQLite ストアと最小スキーマ（`runtimes`、重複排除フィールド、状態バージョンを含む）を実装する。
- ターゲットごとの tmux トポロジーオブザーバーを実装する。
- Reconciler と古い状態の収束ルールを実装する。

終了基準：
- 複数ターゲットのトポロジーおよび状態行が永続化され、クエリ可能であること。
- 古い状態が設定された TTL 内で安全な値に収束すること。
- 決定的な順序付けと重複排除の動作がリプレイテストにより検証されていること。

### Phase 1: 可視性 MVP（Claude + Codex、マルチターゲット）

- ターゲットマネージャー（`add/connect/list/remove`）を実装する。
- Claude フックアダプターを実装する。
- Codex 通知＋ラッパーアダプターを実装する。
- `target-session` デフォルトグルーピング付きのリストコマンド（panes/windows/sessions）を実装する。
- `watch` と `attach` を実装する。
- `agtmuxd API v1` の読み取り/watch 契約を定義・公開する。

終了基準：
- ホストと VM ターゲットをまたいだ Claude/Codex ワークフローで手動ポーリングが不要になること。
- 部分的なターゲット障害時にもリスティングと watch が使用可能であること。
- 可視性レイテンシ目標が達成されること（サポート環境で `p95 <= 2s`）。
- watch jsonl スキーマ契約が互換性テストにより検証されていること。
- attach のフェイルクローズド動作が古いランタイムの統合テストで検証されていること。

### Phase 1.5: コントロール MVP

- フェイルクローズド前提条件チェック付きの `send`、`view-output`、`kill` を実装する。
- 相関イベントを通じたコントロールアクションの監査証跡を追加する。
- コントロールアクション向けの API v1 書き込みエンドポイントを公開する。

終了基準：
- コントロールアクションが古いペイン/ランタイムマッピングに対して安全であること。
- 古いアクション試行がスナップショットガードにより統合テストで拒否されること。
- アクションからイベントへの相関が `action_id` でクエリ可能であること。
- アクションリクエストのべき等リプレイが統合テストで検証されていること。

### Phase 2: Gemini + 信頼性強化

- Gemini アダプターを追加する。
- より強力な再接続ハンドリングとバックオフチューニングを追加する。
- より豊富なフィルター/ソートと JSON スキーマの堅牢化を追加する。

終了基準：
- 3 つすべてのエージェントが安定した状態収束でサポートされること。

### Phase 2.5: アダプター拡張

- Copilot CLI アダプター（v1 ケイパビリティ）を追加する。
- Cursor CLI アダプター（v1 ケイパビリティ）を追加する。
- 両方のオンボーディングにコアエンジンの変更が不要であることを検証する。

終了基準：
- Copilot CLI と Cursor CLI の状態が同じコマンドとサマリーで可視化されること。
- アダプターレジストリとケイパビリティフラグが動作をクリーンに制御すること。

### Phase 3: macOS 常駐アプリ

- 共有デーモン API 上にメニューバーアプリを構築する。
- アクション可能なリストと高速操作を追加する。
- CLI をファーストクラスのインターフェースとして維持する。

終了基準：
- アプリが一目での可視性とコアコントロールアクションを提供すること。

## 9. リスクと軽減策

- リスク：ターミナルテキストパターンに対するパーサーの脆弱性。
  - 軽減策：イベント駆動シグナルを優先し、パーサーはフォールバックとしてのみ使用する。
- リスク：同時実行されるフック/通知からの競合状態。
  - 軽減策：トランザクショナル書き込み＋重複排除キー＋シーケンスチェック。
- リスク：クラッシュや切断後の古い状態。
  - 軽減策：明示的なヘルス遷移と TTL ベースの降格を備えた Reconciler。
- リスク：ターゲット間でのセッション名衝突。
  - 軽減策：デフォルトの `target-session` グルーピング；`session-name` マージは明示的リクエスト時のみ。
- リスク：古いランタイムマッピングが誤ったコントロールターゲットを引き起こす。
  - 軽減策：ランタイムガードとフェイルクローズドのコントロール前提条件。
- リスク：ローカルストアにおける機密接続またはペイロードデータの漏洩。
  - 軽減策：非機密の `connection_ref` のみを保存し、ペイロード編集を適用し、保持 TTL を定義する。
- リスク：UI とコレクター内部の過度な結合。
  - 軽減策：デーモン API 境界を強制する。

## 10. 意思決定とオープンクエスチョン

確定した意思決定：
1. 状態スコープは `host`、`vm1`、`vm2` をまたいで統一される（単一の集約ビュー）。
2. `completed -> idle` のデフォルト降格は 120 seconds（設定可能）。
3. `kill` のデフォルトシグナルは `INT`。
4. 破壊的アクション（`kill`、`remove target`）はデフォルトで確認を要求する。
5. Gemini の戦略は Phase 2 で有効化される場合、まずインタラクティブ CLI を優先し、スクリプトベースの取り込みをサポート拡張パスとする。
6. Copilot CLI と Cursor CLI をインクリメンタルに追加できるよう、アーキテクチャはアダプターファーストを維持しなければならない。
7. デフォルトのセッショングルーピングは `target-session`；クロスターゲットの session-name マージは明示的指定が必要。
8. 全アクションはサーバーサイドのアクションスナップショット検証によりデフォルトでフェイルクローズドである。
9. アクション参照は曖昧性がない必要がある（`runtime:` または完全修飾 `pane:`）。

オープンクエスチョン：
1. Gemini がより強力なネイティブイベントフックを提供した場合、パーサーベースのフォールバックに代わるデフォルトの取り込みパスとすべきか？
2. 非常に大規模な環境では、集約デフォルトビューを `--all-targets` のままにすべきか、それとも現在のターゲットをデフォルトに切り替えるべきか？
