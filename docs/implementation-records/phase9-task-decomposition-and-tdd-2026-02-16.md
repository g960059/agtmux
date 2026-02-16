# AGTMUX Phase 9 タスク分解 + TDD実行計画（2026-02-16）

対象戦略:
- `docs/implementation-records/phase9-main-interactive-tty-strategy-2026-02-16.md`

## 1. 方針

Phase 9 は次の順で進める。

1. Step 0 gate（技術選定 Spike）を完了
2. daemon streaming endpoint + daemon proxy PTY を実装
3. macapp の永続接続クライアントを実装
4. interactive terminal UI を段階導入（feature flag）
5. 回復/降格/fallback を実装
6. AC をテストで閉じる

TDDの原則:
- 各タスクを **RED -> GREEN -> REFACTOR** で進める
- REDは必ず「自動テストの失敗」で確認する
- GREEN後に重複除去・命名改善・責務分離を実施する
- `Manual+CI` の項目は RED ではなく「補助検証」として扱う

## 2. タスク分解（実行順）

| Task ID | Step | Priority | Task | Depends On | First RED Tests | Exit |
| --- | --- | --- | --- | --- | --- | --- |
| TASK-901 | 0 | P0 | streaming endpoint 契約ドラフト作成（attach/detach/write/resize/stream） | - | TC-901 | API契約草案がレビュー通過 |
| TASK-902 | 0 | P0 | daemon proxy PTY local PoC | TASK-901 | TC-902, TC-903 | key-by-key と output stream の成立 |
| TASK-903 | 0 | P0 | terminal emulator PoC（SwiftTerm優先） | TASK-902 | TC-902 | key/IME/resize の成立可否を判断 |
| TASK-904 | 0 | P0 | SLO計測PoC（入力遅延/切替遅延） | TASK-902 | TC-916 | NFR見込みを数値で確認 |
| TASK-911 | 1 | P0 | `TerminalSessionManager` 実装（daemon） | TASK-904 | TC-902, TC-915 | attach/detach lifecycle と GC成立 |
| TASK-912 | 1 | P0 | `/v1/terminal/attach|detach|write|resize` 実装 | TASK-911 | TC-902, TC-903, TC-907 | API契約準拠 + stale guard |
| TASK-913 | 1 | P0 | `/v1/terminal/stream` 実装（長寿命ストリーム） | TASK-912 | TC-902, TC-905 | stream frame 契約を満たす |
| TASK-921 | 2 | P0 | macapp 永続UDSクライアント実装 | TASK-913 | TC-902 | CLI都度起動なしで接続可能 |
| TASK-922 | 2 | P0 | `TerminalSessionController` 実装 | TASK-921 | TC-904, TC-905 | attach/retry/degrade制御が成立 |
| TASK-923 | 2 | P1 | AppViewModel 統合（write系 stale guard 経路統一） | TASK-922 | TC-907 | 誤pane送信を拒否できる |
| TASK-924 | 2 | P0 | resize競合ポリシー実装（tmux複数client前提） | TASK-923 | TC-909 | resize時の競合挙動が仕様どおり |
| TASK-931 | 3 | P0 | interactive terminal view 追加（既存UI並存） | TASK-924 | TC-904 | feature flagで切替可能 |
| TASK-932 | 3 | P1 | fallback導線実装（Open in External Terminal） | TASK-931 | TC-917 | 常時 fallback 実行可能 |
| TASK-941 | 4 | P0 | terminal recovery（指数バックオフ + 自動降格） | TASK-932 | TC-905 | 連続失敗時に確実に降格 |
| TASK-942 | 4 | P1 | kill switch 実装（interactive即無効化） | TASK-941 | TC-914 | Snapshot既定に戻せる |
| TASK-951 | 5 | P0 | daemon contract/integration テスト拡充 | TASK-942 | TC-902, TC-903, TC-907, TC-915 | Goテストで契約固定 |
| TASK-952 | 5 | P0 | macapp unit/UI テスト拡充 | TASK-951 | TC-904, TC-905, TC-917 | Swiftテストで回帰防止 |
| TASK-953 | 5 | P1 | perf/leak テスト実装 | TASK-952 | TC-910, TC-911, TC-912 | NFR判定を自動化 |
| TASK-961 | 6 | P0 | local先行リリース | TASK-953 | TC-911, TC-912 | localでAC達成 |
| TASK-962 | 6 | P1 | ssh段階拡張 | TASK-961 | TC-913 | sshで退行なし |

## 3. RED->GREEN->REFACTOR 具体ループ

| フェーズ | 目的 | 実施内容 |
| --- | --- | --- |
| RED | 失敗を先に固定 | 新規テストを追加し、現実装で失敗を確認 |
| GREEN | 最小実装で通す | 仕様を満たす最短実装のみ投入 |
| REFACTOR | 保守性改善 | 重複除去、責務分離、命名改善、ログ整備 |

適用ルール:
- 1タスクあたり最大3テストから開始
- GREENで通るまで新機能追加をしない
- REFACTOR後に必ず全関連テストを再実行

## 4. 先に作るREDテスト（優先順）

1. TC-902: `terminal/attach + stream` 契約
2. TC-903: key-by-key write round-trip
3. TC-907: write系 stale guard 拒否
4. TC-916: Step0 SLO feasibility smoke
5. TC-904: 50ms間隔100回 pane切替の一致性
6. TC-905: session establish/stream/write 失敗時の自動降格

補助検証（RED対象外）:
- TC-906: 外部terminal fallback UX
- TC-908: IME/修飾キー 実機確認
- TC-913: ssh parity 実機確認

## 5. テスト実行コマンド

Go:
```bash
go test ./internal/daemon -run 'Terminal|Attach|Stream|Stale' -count=1
go test ./... -count=1
```

Swift:
```bash
cd macapp && swift test --filter Terminal
cd macapp && swift test
```

統合確認:
```bash
go test ./... && (cd macapp && swift test)
```

## 6. Gate判定

Step 0 gate を通過する条件:
- TC-901, TC-902, TC-903, TC-916 が期待どおり
- NFR-1/NFR-2 の見込み値（PoC計測）が提示される
- 方式選定（endpoint + proxy PTY）がレビュー承認される

Phase 9 実装開始条件:
- Step 0 gate pass
- Critical/High の未解決レビュー指摘がない
