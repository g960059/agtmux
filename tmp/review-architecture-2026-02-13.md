# AGTMUX v0.5 アーキテクチャ・技術設計レビュー報告書

**レビュー対象**: agtmux-spec.md, implementation-plan.md, tasks.md, test-catalog.md
**レビュー日**: 2026-02-13
**レビュアー**: Opus Agent #1 (アーキテクチャ・技術設計)

---

## 総合評価

仕様書全体として、非常に高い設計品質を持つドキュメント群です。状態モデルの正式化、イベント順序制御の厳密な定義、fail-closed原則の一貫した適用、そしてアダプタパターンによる拡張性の確保は、この種のシステムとしては模範的な設計方針です。一方で、以下に挙げるいくつかの構造的・設計的な課題が存在します。

---

## 1. 全体アーキテクチャの妥当性

### [Major] M-01: デーモン（agtmuxd）のプロセスモデルとAPIトランスポートが未定義

仕様書ではデーモン `agtmuxd` がCLIとmacOSアプリ双方にAPIを提供すると記載されている（Section 7.9）が、APIのトランスポート層が明示されていません。REST over HTTP（Unixドメインソケット or TCP）なのか、gRPCなのか、あるいはプロセス内IPC（stdin/stdout）なのかが不明です。

- **影響**: Phase 0でデーモン境界を実装する（TASK-031）にもかかわらず、通信プロトコルが未決定のため、実装者が独自判断で決めることになり、後工程での手戻りリスクが大きい。
- **推奨**: Phase 0のEntry Criteriaに「APIトランスポートの決定」を追加。Unixドメインソケット + HTTP/JSON が最もシンプルで、CLIからもmacOSアプリからもアクセス容易。

### [Minor] M-02: tmux依存の代替検討が記載されていない

tmuxは適切な選択であるものの、Non-Goalsに「Replacing tmux itself」と記載しつつも、tmux自体のバージョン要件（最低バージョン、`server_boot_id` の利用可否など）が明記されていません。`tmux_server_boot_id` はtmux 3.2以降でしかサポートされていない可能性があります。

- **推奨**: tmuxの最低サポートバージョンを明記し、`server_boot_id` 取得のフォールバック戦略を定義する。

### [Info] I-01: TypeScript選定の根拠が未記載

プロジェクトルートにはまだソースコードが存在せず、仕様書にもTypeScriptという記述はあるが選定理由の記載がない。Node.jsランタイムのメモリフットプリントはNFR-3（低CPU/メモリオーバーヘッド）との整合性を検証すべきです。60ペインの監視とSQLite操作を行うデーモンとして、Go や Rust のほうがリソース効率面では優位の可能性があります。

---

## 2. コンポーネント分割

### [Major] M-03: State EngineとReconcilerの責務境界が曖昧

仕様書では以下のコンポーネントが状態遷移に関与しますが、それぞれの所有境界が不明確です。

- **State Engine**: アダプタイベントとtmux観測をマージしてcanonical stateを生成
- **Reconciler**: 定期tmuxスキャン、stale demotion、target health遷移
- **Ordering/Dedupe**: イベント適用順序の決定

Reconcilerが「stale demotion and target health transitions are reconciler-owned」（Section 7.4.1）と記載される一方、State Engineが「merges adapter events and tmux observations into canonical state」（Section 7.1）とされています。`completed -> idle` のデモーション判断はState Engineの状態遷移ロジックなのか、Reconcilerのジョブなのかが明確でない。

- **影響**: 実装時にReconcilerとState Engineが相互に状態を書き換える形になり、デッドロックや競合状態のリスクが生じる。
- **推奨**: 状態書き込みのオーナーシップを「State Engineのみが`states`テーブルを書き換える」と明確化し、Reconcilerは「合成イベントを生成してState Engineに投入する」形に統一する。

### [Minor] M-04: Collector と Tmux Observer の関係が不明

Section 7.1で「Collector (per target)」と「Tmux Observer」が別コンポーネントとして列挙されていますが、CollectorがTmux Observerを包含するのか、並列なのかが不明です。

- **推奨**: コンポーネント図（テキストベースでもよい）を追加し、依存関係を明示する。

### [Info] I-02: Adapter RegistryとAdapter Contractの二重管理リスク

`adapters`テーブル（Section 7.3）に`capabilities` (JSON)を格納しつつ、AdapterContract (Section 7.2.3) でも能力宣言する設計になっている。DBの値とコードの値が乖離した場合のauthoritative sourceが未定義。

---

## 3. 通信設計

### [Critical] C-01: SSH接続のライフサイクル管理が未定義

マルチターゲット設計（host, vm1, vm2）の中核であるSSH接続について、以下が未定義です。

- SSH接続の確立・維持・再接続のライフサイクル
- SSH接続プーリングの有無（ControlMaster利用等）
- SSH経由でのtmuxコマンド実行のタイムアウト・リトライ戦略
- TargetExecutorがSSH接続をどのように抽象化するか

**影響**: NFR-1（2秒以内の可視化遅延）の達成に直結する。SSHコマンドの都度接続では60ペインの監視で2秒を超過する可能性が非常に高い。また、NFR-7（部分障害時の動作継続）の実装も、SSH接続管理の設計無しには進められない。

- **推奨**: TargetExecutor仕様にSSH接続管理戦略を明記する。SSH ControlMaster/ControlPersistの利用、または常時接続のmultiplexed SSHセッションの採用を検討する。

### [Major] M-05: イベント伝搬パスが複数経路あり、整合性の保証が複雑

イベントの発生源が4種類（hook, notify, wrapper, poller）あり、それぞれが異なるタイミングで到着する。さらに`event_inbox`の`pending_bind`パスを経由するものとしないものが混在します。

```
Hook Event ---> (runtime_id既知) ---> events テーブル直行
Wrapper Event -> (runtime_id未知) ---> event_inbox -> bind resolver -> events テーブル
```

この二重パスにおいて、同一ランタイムに対するイベントの到着順序逆転が発生した場合（例: hookイベントが先にeventsに入り、その直後にwrapperの`started`イベントがinboxで解決される場合）、source cursor更新のタイミングによっては本来受理すべきイベントがdropされる可能性があります。

- **推奨**: 全イベントを一旦event_inboxに投入し、統一パスで処理する設計を検討する。パフォーマンスが懸念であれば、runtime_id既知のイベントに対するfast pathを設けつつ、cursor更新はatomicに行う制約を明記する。

### [Major] M-06: Watch Stream のバックプレッシャー機構が未定義

`watch --format jsonl`（Section 7.5.1）はストリーミングAPIだが、クライアントの消費速度が遅い場合のバックプレッシャー戦略が未定義です。cursorベースのresume機構は存在するが、アクティブストリーム中にバッファが溢れた場合の挙動が不明。

- **推奨**: ストリームバッファの上限とoverflow時の挙動（接続切断 or resetイベント送出）を定義する。

---

## 4. スケーラビリティ

### [Major] M-07: Reconcilerのポーリング間隔とターゲット数のスケーリング

Reconcilerが2秒間隔で全アクティブペインに対してtmuxスキャンを実行する設計（Section 7.4.1）です。3ターゲット x 60ペインの場合、2秒ごとに最大180回のtmuxコマンド（SSH経由含む）が発生する可能性があります。

- **影響**: ターゲット数やペイン数が増加した場合、2秒の予算内にスキャン完了できなくなる。
- **推奨**: tmuxのlist-panesコマンドでバッチ取得する設計を明記する（1ターゲットあたり1コマンドで全ペイン情報取得可能）。また、ターゲットごとのスキャンを並行実行する戦略も記載すべき。

### [Minor] M-08: SQLiteの書き込み同時実行制限

SQLiteはWALモードでも単一ライターの制約があります。イベント高頻度到着時（NFR-4のduplicate stormシナリオ等）に書き込みが直列化され、ボトルネックになる可能性があります。

- **推奨**: ベンチマークプロファイル（TC-044）にSQLite書き込みスループットの計測を含める。バッチ書き込み戦略（N件をまとめてトランザクション）も検討する。

### [Info] I-03: macOSアプリ向けAPIの接続方式の拡張性

Phase 3でmacOSアプリが同一デーモンAPIを利用する設計だが、将来的にmacOSアプリからリモートのデーモンに接続するシナリオ（例: 手元のMacからVM上のagtmuxdに接続）が想定されていない。APIがUnixドメインソケット前提だと、このシナリオへの拡張にはSSHトンネル等の追加インフラが必要になる。

---

## 5. セキュリティ

### [Major] M-09: SSH認証情報の取り扱いが不十分

`targets.connection_ref`に「non-secret reference (ssh host alias)」のみ格納する設計（Section 7.3.1）は妥当だが、以下が未定義です。

- TargetExecutorがSSH接続を確立する際の認証方式（鍵認証, agent forwarding等）の制約
- SSH接続のcredentialがメモリ上にどの程度残存するかの管理方針
- デーモンプロセスの実行ユーザーと権限の推奨構成

### [Major] M-10: sendコマンドのインジェクションリスク

`agtmux send <ref> --text <text>` はtmux `send-keys` を介してペインにテキストを送信する。TargetExecutorがSSH経由でtmuxコマンドを実行する場合、入力テキストのエスケープ処理が不十分だとコマンドインジェクションが発生する可能性がある。

- **影響**: 悪意あるテキスト入力によって、ターゲットホスト上で任意コマンドが実行される恐れがある。
- **推奨**: sendコマンドのテキストサニタイズ戦略をspec levelで明記する。`--paste`モードでのtmux paste-buffer経由が安全だが、通常の`--text`モードでも安全なエスケープを保証する必要がある。

### [Minor] M-11: デーモンAPIの認証・認可が未定義

agtmuxd API v1（Section 7.9）にアクセス制御の記載がない。ローカルのみの利用前提であっても、Unixドメインソケットのパーミッション制御や、TCP公開時の認証トークンの必要性は明記すべきです。

---

## 6. 技術的負債のリスク

### [Critical] C-02: 仕様の過剰設計と実装開始前の状態のギャップ

プロジェクトルートには `docs/` と `tmp/`（過去のレビュー結果）のみが存在し、ソースコードはゼロの状態です。にもかかわらず、仕様書はv0.5として以下のような高度に詳細化された契約を定義しています。

- 13テーブル・40以上のカラムのDBスキーマ
- 12種のエラーコード
- BNF文法によるアクション参照仕様
- Watch JSONLストリームのcursorベースresume仕様
- Action snapshot + idempotency + fail-closed の三重安全機構

**リスク**: これだけの詳細仕様をコード0行の段階で固定すると、実装過程で仕様と実装の乖離が急速に拡大し、仕様の形骸化または過度な仕様追従コストが発生する。実装段階で判明する設計課題（例: event_inboxのbindフローの実用的なパフォーマンス、watchストリームのメモリ特性等）にフィードバックできない「仕様先行のウォーターフォール」に陥る危険がある。

- **推奨**: Phase 0の実装をプロトタイプ段階と位置付け、Phase 0完了時にspec feedbackサイクルを明示的に設ける。Change Control（Section 8）は存在するが、「実装からの学びによるspec改訂」を積極的に促す文化的ガードレールが必要。

### [Major] M-12: event_inbox の pending_bind フローの複雑性

`event_inbox`テーブルとbind resolver（Section 7.4.1）は概念的には正しいが、実装複雑度が非常に高い。pending_bind -> bind候補検索 -> pid/start_hint照合 -> TTL期限管理 -> dropped_unbound理由コード付与、というフロー全体が正しく動作するためのテストマトリクスは膨大になります。

TC-008, TC-009でカバーされていますが、2テストケースでは到底不十分です。pid照合の境界条件、start_hintのskew、複数ペインの同時reuse、bind中のpane epoch変更など、組み合わせ爆発が懸念されます。

- **推奨**: pending_bindのproperty testを追加する（TC-008/009をpropertyテスト層に昇格）。あるいは、Phase 0では全イベントにruntime_idの付与をアダプタ側に強制し、pending_bind不要の設計で始め、やむを得ないケースのみPhase 2以降で対応する。

### [Major] M-13: タスク定義において Phase 0 の TASK-035（adapter registry）がPhase 2に配置されている不整合

`tasks.md`のTASK-035は「Implement adapter registry capability-driven dispatch」で Phase 2 に配置されているが、Phase 1 でClaude/Codexアダプタを実装する際にアダプタ登録・ディスパッチの仕組みが必要になる。Phase 1のTASK-010, TASK-011がアダプタを実装するが、registryなしでどのようにディスパッチするのかが不明。

- **影響**: Phase 1でハードコードされたアダプタ選択ロジックが作られ、Phase 2でregistryに移行する際にリファクタリングが必要になる。
- **推奨**: adapter registryの最小実装（登録とdispatch）をPhase 0またはPhase 1に前倒しする。

### [Minor] M-14: テストカタログのE2Eテスト比率が高くCI負荷が懸念

53件のテスト中、E2Eテストが11件、Integrationテストが18件、Contractテストが12件で、軽量なUnitテストは3件のみ。E2Eテストは実行にtmux環境とSSH接続のセットアップが必要で、CIの実行時間と安定性に影響する。

- **推奨**: Unit/Propertyテスト層を厚くし、E2Eテストの一部をIntegration層に降格できないか検討する。特にTC-014（Target manager basic flow）やTC-024（Encoded target-session round-trip）はUnit層でカバー可能な部分が多い。

### [Minor] M-15: runtime_id のsha256導出が過剰

Section 7.2.4で `runtime_id = sha256(target_id + tmux_server_boot_id + pane_id + pane_epoch + agent_type + started_at_ns)` とされているが、sha256はハッシュ衝突耐性が要件でない限り不要。UUIDv7やULIDで十分であり、デバッグ時の可読性も向上する。sha256導出は構成要素の変更時に同一runtime_idが再生成される「再現性」が利点だが、この再現性が実際に必要となるシナリオが仕様書に記載されていない。

---

## 7. ドキュメント間の整合性

### [Major] M-16: implementation-plan.md の Phase 0 Exit Criteria と test-catalog.md の Phase 0 Gate Bundle の不一致

- implementation-plan Section 5 Phase 0 Exit Criteria: `TC-011, TC-012, TC-013, TC-040, TC-041`
- test-catalog Section 3 Phase 0 close: `TC-001 ~ TC-013, TC-040, TC-041, TC-049, TC-050`

test-catalogの方がTC-049（Duplicate-storm convergence）とTC-050（Debug raw payload prohibition）を追加で含んでいます。implementation-planとtest-catalogで齟齬があり、どちらがauthoritativeか不明です。

### [Minor] M-17: tasks.md のTASK-027, TASK-028 の依存先にTASK-035が含まれるが、TASK-035自体がPhase 2

TASK-027（Copilot adapter, Phase 2.5）とTASK-028（Cursor adapter, Phase 2.5）が`Depends On: TASK-035`（adapter registry, Phase 2）を参照しており、依存関係自体は正しいが、TASK-035のPhase内優先度がP0であるべきところP0でない可能性がある。

---

## 8. 見落とされている設計考慮事項

### [Major] M-18: デーモンの起動・停止・ヘルスチェック仕様が未定義

`agtmuxd`のライフサイクル管理が仕様書に存在しません。

- デーモンの起動方法（手動起動, launchd, systemd）
- PIDファイル管理
- CLIからデーモンへの到達性確認（`agtmux status` 等）
- デーモンクラッシュ時の自動再起動戦略
- グレースフルシャットダウン時のwatch stream切断処理

TC-051（Watch continuity after daemon restart）でデーモン再起動テストは存在するが、再起動の方法自体が未定義です。

### [Minor] M-19: ログ・オブザーバビリティ戦略が未定義

デーモンのログレベル、構造化ログフォーマット、メトリクス取得の方針がない。トラブルシューティング時に不可欠であり、Phase 0で最低限の方針を決めるべきです。

---

## 指摘事項サマリ

| 重要度 | ID | カテゴリ | 概要 |
|---|---|---|---|
| Critical | C-01 | 通信設計 | SSH接続ライフサイクル管理が未定義。NFR-1達成に直結 |
| Critical | C-02 | 技術的負債 | コード0行に対する過剰詳細仕様。仕様形骸化リスク |
| Major | M-01 | アーキテクチャ | デーモンAPIトランスポート未定義 |
| Major | M-03 | コンポーネント分割 | State EngineとReconcilerの責務境界が曖昧 |
| Major | M-05 | 通信設計 | イベント伝搬の二重パスによる順序逆転リスク |
| Major | M-06 | 通信設計 | Watch Streamのバックプレッシャー未定義 |
| Major | M-07 | スケーラビリティ | Reconcilerポーリングのスケーリング問題 |
| Major | M-09 | セキュリティ | SSH認証情報の取り扱い方針不足 |
| Major | M-10 | セキュリティ | sendコマンドのインジェクションリスク |
| Major | M-12 | 技術的負債 | event_inbox pending_bindフローの複雑性 |
| Major | M-13 | 技術的負債 | adapter registryのPhase配置不整合 |
| Major | M-16 | ドキュメント整合性 | Phase 0 Exit CriteriaとGate Bundleの不一致 |
| Major | M-18 | アーキテクチャ | デーモンのライフサイクル管理仕様の欠如 |
| Minor | M-02 | アーキテクチャ | tmux最低バージョン要件未記載 |
| Minor | M-04 | コンポーネント分割 | CollectorとTmux Observerの関係不明 |
| Minor | M-08 | スケーラビリティ | SQLite書き込み同時実行の制限 |
| Minor | M-11 | セキュリティ | デーモンAPI認証・認可未定義 |
| Minor | M-14 | 技術的負債 | E2Eテスト比率が高くCI負荷懸念 |
| Minor | M-15 | 技術的負債 | runtime_idのsha256導出が過剰 |
| Minor | M-17 | ドキュメント整合性 | タスク依存関係の優先度の曖昧さ |
| Minor | M-19 | アーキテクチャ | ログ・オブザーバビリティ戦略の欠如 |
| Info | I-01 | アーキテクチャ | TypeScript選定根拠が未記載 |
| Info | I-02 | コンポーネント分割 | Adapter RegistryとDBの二重管理リスク |
| Info | I-03 | スケーラビリティ | macOSアプリからのリモートデーモン接続 |

---

## 対応優先度の推奨

**Phase 0実装開始前に解決すべき項目** (blocking):
1. **C-01**: SSH接続管理戦略の策定
2. **M-01**: APIトランスポートの決定
3. **M-03**: State Engine / Reconciler の責務境界の明確化
4. **M-18**: デーモンライフサイクル管理の定義

**Phase 0実装中に並行して対処すべき項目**:
5. **C-02**: プロトタイプフィードバックサイクルの導入
6. **M-05**: イベント伝搬パスの統一または安全性保証の強化
7. **M-10**: sendコマンドのサニタイズ戦略
8. **M-13**: adapter registryの前倒し検討
9. **M-16**: ドキュメント間の整合性修正

**Phase 1以降で対処可能な項目**:
10. その他のMajor/Minor/Info項目
