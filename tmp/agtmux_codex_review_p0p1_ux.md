判定: **Stop**（自動化契約としては未完成。実装すると再開処理と異常系で壊れやすいです）

**Blocking issues（重大度順）**
1. **Critical / 高確度**  
`--since` の型が仕様内で矛盾しています。CLIは timestamp、watch契約とAPIは sequence/cursor です。  
参照: `docs/agtmux-spec.md:415`, `docs/agtmux-spec.md:466`, `docs/agtmux-spec.md:537`  
破綻リスク: 自動化クライアントの再開位置が不一致になり、取りこぼし/重複が発生。  
修正: watch の再開パラメータを `cursor` に一本化（timestamp は別フラグに分離）。

2. **Critical / 高確度**  
`sequence` が「per stream」なのに stream 同一性とリセット規約が未定義です。  
参照: `docs/agtmux-spec.md:460`, `docs/agtmux-spec.md:466`  
破綻リスク: daemon再起動後に sequence 再利用され、再開が不正（欠落/重複）。  
修正: `stream_id` と `cursor=<stream_id>:<sequence>` を必須化し、`E_CURSOR_EXPIRED`/`reset` イベントを定義。

3. **High / 高確度**  
エラーコードは列挙のみで、機械処理に必要な「エラーオブジェクト」「HTTP/CLI終了コード」「retriable」が未定義です。  
参照: `docs/agtmux-spec.md:552`, `docs/agtmux-spec.md:558`, `docs/agtmux-spec.md:560`  
破綻リスク: リトライ判定や恒久失敗判定が実装者依存になる。  
修正: 統一エラー envelope と code->status->exit code マッピングを規定。

4. **High / 高確度**  
部分失敗時の返却契約が不足しています（NFRはあるがJSON仕様に落ちていない）。  
参照: `docs/agtmux-spec.md:92`, `docs/agtmux-spec.md:446`, `docs/agtmux-spec.md:588`  
破綻リスク: クライアントが「全件成功」と誤認。  
修正: `partial`, `target_errors[]`, `requested_targets`, `responded_targets` を read/watch に必須追加。

5. **High / 高確度**  
参照文法とフィルタ文法のエンコード規約が不整合です。`pane:` は percent-encode必須だが `--target-session <target>/<session>` は未定義。  
参照: `docs/agtmux-spec.md:408`, `docs/agtmux-spec.md:423`, `docs/agtmux-spec.md:425`, `docs/agtmux-spec.md:432`  
破綻リスク: `/` を含む session 名で絞り込み・アクション参照が壊れる。  
修正: `target-session` も `<target>/<session-enc>` に統一し、一覧JSONに canonical `ref` を含める。

6. **High / 高確度**  
watch jsonl の delta 形状が曖昧です（`items` と `op` の関係が未規定）。  
参照: `docs/agtmux-spec.md:463`, `docs/agtmux-spec.md:465`  
破綻リスク: クライアントごとにパース実装が分岐し互換性崩壊。  
修正: `delta` は `changes[]` を必須化（`upsert` は `item` 必須、`delete` は `identity` のみ）。

**Non-blocking issues**
1. **Medium / 高確度**  
非TTY時の確認プロンプト動作が未定義です（`kill`, `target remove`）。  
参照: `docs/agtmux-spec.md:405`, `docs/agtmux-spec.md:508`  
修正: 非TTYかつ `--yes` 無しは即 `E_CONFIRM_REQUIRED` で失敗（待機禁止）。

2. **Medium / 中確度**  
timestampフィールド命名とテーブル名に微不整合があります。  
参照: `docs/agtmux-spec.md:457`, `docs/agtmux-spec.md:530`, `docs/agtmux-spec.md:298`, `docs/agtmux-spec.md:486`  
修正: watchに `generated_at` を含めるか read要件をwatch除外。`action_snapshot`/`action_snapshots` を統一。

**具体修正文案（そのまま仕様に入れられる形）**
```md
Watch cursor contract:
- CLI: `agtmux watch ... [--cursor <cursor>] [--from snapshot|latest] [--once]`
- API: `GET /v1/watch?scope=<...>&cursor=<cursor>`
- `cursor` format: `<stream_id>:<sequence>`
- On daemon restart (stream_id changed), server MUST emit `type=reset` then `type=snapshot`.
- Invalid cursor -> `E_CURSOR_INVALID`
- Expired cursor (outside retention) -> `E_CURSOR_EXPIRED`
```

```json
{"schema_version":"1.0","stream_id":"w_01J...","cursor":"w_01J...:120","type":"snapshot","scope":"panes","generated_at":"2026-02-13T10:00:00Z","emitted_at":"2026-02-13T10:00:00Z","filters":{},"summary":{"total":2},"items":[{"identity":{"target":"host","session_name":"proj","window_id":"@1","pane_id":"%3"},"ref":"pane:host/proj/@1/%3","runtime_ref":"runtime:rt_...","state":"running"}]}
{"schema_version":"1.0","stream_id":"w_01J...","cursor":"w_01J...:121","type":"delta","scope":"panes","generated_at":"2026-02-13T10:00:01Z","emitted_at":"2026-02-13T10:00:01Z","changes":[{"op":"upsert","identity":{"target":"host","session_name":"proj","window_id":"@1","pane_id":"%3"},"item":{"state":"waiting_input"}},{"op":"delete","identity":{"target":"host","session_name":"proj","window_id":"@2","pane_id":"%7"}}]}
```

```md
Reference/filter grammar:
<target-session> ::= <target> "/" <session-enc>
--target-session accepts only <target-session>.
List JSON for panes MUST include `ref` and `runtime_ref` (when available), so clients do not re-encode manually.
```

```json
{
  "schema_version":"1.0",
  "error":{
    "code":"E_CURSOR_EXPIRED",
    "message":"cursor is older than retention window",
    "category":"transient",
    "retriable":true,
    "http_status":409,
    "details":{"latest_cursor":"w_01J...:8901"}
  }
}
```

**不足テスト（優先度高）**
1. daemon再起動を跨ぐ watch resume（cursor の欠落/重複ゼロ保証）。  
2. `E_CURSOR_EXPIRED` 発生時の `reset + snapshot` 復帰シナリオ。  
3. `session` に `/`, `%`, 空白, UTF-8 を含むときの `ref`/`--target-session` round-trip。  
4. 部分失敗時の `partial` 契約と `--strict-targets`（導入時）動作。  
5. 非TTYで `kill`/`target remove` を `--yes` なし実行したときの即時失敗。
