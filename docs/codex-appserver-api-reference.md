# Codex App Server API Reference (authoritative; MUST read before any Codex implementation)

> Source: https://developers.openai.com/codex/app-server/
> Last synced: 2026-02-26

**IMPORTANT**: agtmux は公式 Codex App Server API を使用する。独自プロトコル（capture-based NDJSON 抽出等）は
App Server が利用不可の場合のフォールバックとしてのみ使用する。新機能実装時は必ずこのリファレンスを参照すること。

## 1. Protocol Overview

- **Transport**: JSON-RPC 2.0 over stdio (default, newline-delimited JSON) / WebSocket (experimental)
- **起動コマンド**: `codex app-server` (stdio) / `codex app-server --listen ws://IP:PORT` (WebSocket)
- **メッセージ形式**: 全メッセージに `"jsonrpc": "2.0"` フィールドが必須

### メッセージ種別

| 種別 | 特徴 | 例 |
|------|------|-----|
| Request | `method` + `params` + `id` | `{"jsonrpc":"2.0","method":"thread/list","id":1,"params":{}}` |
| Response | `id` + `result` or `error` | `{"jsonrpc":"2.0","id":1,"result":{...}}` |
| Notification | `method` + `params`, **`id` なし** | `{"jsonrpc":"2.0","method":"turn/started","params":{...}}` |

## 2. Connection Lifecycle (Initialize Handshake)

全リクエストの前に必須。この手順を省略すると "Not initialized" エラー。

```
Step 1 (Client → Server): initialize request
{
  "jsonrpc": "2.0",
  "method": "initialize",
  "id": 0,
  "params": {
    "clientInfo": {
      "name": "agtmux",
      "title": "agtmux v5",
      "version": "0.1.0"
    },
    "capabilities": {}
  }
}

Step 2 (Server → Client): initialize response
{
  "jsonrpc": "2.0",
  "id": 0,
  "result": { "userAgent": "..." }
}

Step 3 (Client → Server): initialized notification (id なし)
{
  "jsonrpc": "2.0",
  "method": "initialized",
  "params": {}
}
```

### initialize params

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `clientInfo.name` | string | required | Client identifier |
| `clientInfo.title` | string | optional | Display name |
| `clientInfo.version` | string | optional | Version string |
| `capabilities.experimentalApi` | bool | optional | Gates experimental methods |
| `capabilities.optOutNotificationMethods` | string[] | optional | Suppress specific notifications |

## 3. Thread Methods

### `thread/list` — ページング付きスレッド一覧

agtmux の主要ポーリング対象。`cwd` フィルタで pane 対応が可能。

```json
{
  "jsonrpc": "2.0", "method": "thread/list", "id": 1,
  "params": {
    "cursor": null,
    "limit": 50,
    "sortKey": "updated_at",
    "cwd": "/path/to/project",
    "sourceKinds": [],
    "archived": false
  }
}
```

**Response**:
```json
{
  "jsonrpc": "2.0", "id": 1,
  "result": {
    "data": [
      {
        "id": "thr_abc",
        "preview": "Create a TUI",
        "modelProvider": "openai",
        "createdAt": 1730831111,
        "updatedAt": 1730831111,
        "name": "TUI prototype",
        "status": { "type": "idle" }  // ⚠ May be OMITTED — see note below
      }
    ],
    "nextCursor": "opaque-token-or-null"
  }
}
```

**params schema**:

| Field | Type | Description |
|-------|------|-------------|
| `cursor` | string? | Pagination token |
| `limit` | number? | Page size |
| `sortKey` | string? | `"created_at"` or `"updated_at"` |
| `cwd` | string? | **Exact working directory match** (T-119 pane correlation に使用) |
| `sourceKinds` | string[]? | `cli`, `vscode`, `exec`, `appServer`, `subAgent*`, `unknown` |
| `modelProviders` | string[]? | Filter by provider |
| `archived` | bool? | Show archived only |

> **⚠ IMPORTANT: `status` field may be omitted (v0.104.0+)**
>
> The API documentation shows `"status": { "type": "idle" }` in thread objects, but the
> real Codex App Server (v0.104.0) frequently omits the `status` field entirely from
> `thread/list` responses. The `status` is only guaranteed in:
> - `thread/status/changed` notifications
> - `thread/read` responses
>
> **agtmux handling**: When `status` is absent, default to `"idle"` (a listed thread is
> at minimum available/loaded). Do NOT default to `"unknown"` — this causes all Codex panes
> to show `ActivityState::Unknown`.

### `thread/start` — 新規スレッド作成

```json
{
  "jsonrpc": "2.0", "method": "thread/start", "id": 2,
  "params": {
    "model": "gpt-5.1-codex",
    "cwd": "/path/to/project",
    "approvalPolicy": "never",
    "sandbox": "workspaceWrite"
  }
}
```

**Response**: `{ "thread": { "id": "thr_123", "preview": "", "modelProvider": "openai", "createdAt": ... } }`

### `thread/resume` — 既存スレッド再開

```json
{ "jsonrpc": "2.0", "method": "thread/resume", "id": 3, "params": { "threadId": "thr_123" } }
```

### `thread/read` — スレッドデータ取得 (subscribe なし)

```json
{ "jsonrpc": "2.0", "method": "thread/read", "id": 4, "params": { "threadId": "thr_123", "includeTurns": true } }
```

**Response**: thread object with optional `turns` array.

### `thread/loaded/list` — メモリ上のスレッド ID 一覧

```json
{ "jsonrpc": "2.0", "method": "thread/loaded/list", "id": 5 }
```

**Response**: `{ "data": ["thr_123", "thr_456"] }`

### `thread/fork` / `thread/archive` / `thread/unarchive` / `thread/compact/start` / `thread/rollback`

| Method | Key params | Description |
|--------|-----------|-------------|
| `thread/fork` | `threadId` | Branch thread |
| `thread/archive` | `threadId` | Archive (emits `thread/archived`) |
| `thread/unarchive` | `threadId` | Restore (emits `thread/unarchived`) |
| `thread/compact/start` | `threadId` | Context compaction |
| `thread/rollback` | `threadId`, `numberOfTurns` | Drop last N turns |

## 4. Turn Methods

### `turn/start` — ターン開始

```json
{
  "jsonrpc": "2.0", "method": "turn/start", "id": 10,
  "params": {
    "threadId": "thr_123",
    "input": [{ "type": "text", "text": "Fix the bug in auth.rs" }],
    "model": "gpt-5.1-codex",
    "effort": "medium"
  }
}
```

**input item types**: `text`, `image`, `localImage`, `skill`, `mention`

**Response**: `{ "turn": { "id": "turn_456", "status": "inProgress", "items": [] } }`

### `turn/steer` — 実行中ターンに追加入力

```json
{
  "jsonrpc": "2.0", "method": "turn/steer", "id": 11,
  "params": { "threadId": "thr_123", "expectedTurnId": "turn_456", "input": [{ "type": "text", "text": "Use TypeScript" }] }
}
```

### `turn/interrupt` — ターンキャンセル

```json
{
  "jsonrpc": "2.0", "method": "turn/interrupt", "id": 12,
  "params": { "threadId": "thr_123", "turnId": "turn_456" }
}
```

## 5. Notifications (Server → Client)

agtmux が受信して `CodexRawEvent` に変換する通知。

### Thread lifecycle

| Method | Params | agtmux event_type |
|--------|--------|-------------------|
| `thread/started` | `{ thread: { id } }` | (spawn tracking) |
| `thread/status/changed` | `{ threadId, status: { type, activeFlags? } }` | `thread.{type}` |
| `thread/archived` | `{ threadId }` | — |
| `thread/unarchived` | `{ threadId }` | — |
| `thread/tokenUsage/updated` | `{ usage }` | — |

### Thread runtime status types

| Status | Description | agtmux mapping |
|--------|-------------|----------------|
| `notLoaded` | On disk, not in memory | — |
| `idle` | Loaded, ready | `thread.idle` |
| `active` | In-flight turn | `thread.active` |
| `systemError` | Internal error | `thread.systemError` |

`active` status may include `activeFlags`: `["waitingOnApproval"]`

### Turn lifecycle

| Method | Params | agtmux event_type |
|--------|--------|-------------------|
| `turn/started` | `{ turn: { id } }` | `turn.started` |
| `turn/completed` | `{ turn: { id, status } }` | `turn.{status}` |
| `turn/diff/updated` | `{ threadId, turnId, diff }` | — |
| `turn/plan/updated` | `{ turnId, explanation, plan }` | — |

Turn status values: `inProgress`, `completed`, `interrupted`, `failed`

### Item lifecycle (streaming)

| Method | Description |
|--------|-------------|
| `item/started` | Item creation |
| `item/completed` | Item finished |
| `item/agentMessage/delta` | Agent text streaming (opt-out 可能) |
| `item/commandExecution/outputDelta` | Command output streaming |
| `item/fileChange/outputDelta` | File change streaming |
| `item/plan/delta` | Plan text streaming |
| `item/reasoning/summaryTextDelta` | Reasoning summary streaming |

### Approval requests (Server → Client requests with `id`)

| Method | Description |
|--------|-------------|
| `item/commandExecution/requestApproval` | Command approval |
| `item/fileChange/requestApproval` | File change approval |

Response options: `"accept"`, `"acceptForSession"`, `"decline"`, `"cancel"`

## 6. Configuration & Account Methods

| Method | Description |
|--------|-------------|
| `config/read` | Fetch effective configuration |
| `config/value/write` | Update single config key |
| `config/batchWrite` | Atomic multi-key update |
| `config/mcpServer/reload` | Reload MCP server config |
| `configRequirements/read` | Fetch admin requirements |
| `account/read` | Current account info |
| `account/login/start` | Begin login (apiKey / chatgpt / chatgptAuthTokens) |
| `account/login/cancel` | Cancel pending login |
| `account/logout` | Sign out |
| `account/rateLimits/read` | ChatGPT rate limits |
| `skills/list` | Discover skills by cwds |
| `skills/config/write` | Enable/disable skill |
| `app/list` | List available apps |
| `mcpServer/oauth/login` | MCP OAuth login |
| `mcpServerStatus/list` | MCP server status |

## 7. Error Codes

| Code | Message |
|------|---------|
| `-32001` | Server overloaded (WebSocket, retry with backoff) |
| `-32600` | Invalid Request |
| `-32601` | Method not found |
| `-32602` | Invalid params |

Application errors: `"Not initialized"`, `"Already initialized"`, `"requires experimentalApi capability"`

Turn error extensions: `error.codexErrorInfo`, `error.additionalDetails`, `error.httpStatusCode`

## 8. agtmux 実装方針

### Primary path: App Server stdio client (`CodexAppServerClient`)

1. `codex app-server` を child process として spawn (stdin/stdout piped)
2. Initialize handshake (10s timeout)
3. 毎 poll tick で `thread/list` を呼び出し、status 変化を検出
4. Notification (`turn/started`, `turn/completed`, `thread/status/changed`) を `CodexRawEvent` に変換
5. `codex_source.ingest()` で gateway に流す

### Fallback path: Capture-based NDJSON extraction

App Server が利用不可の場合（`codex` 未インストール、認証失敗等）のみ使用:
1. tmux capture から `codex exec --json` の NDJSON 出力を parse
2. Content-based fingerprint dedup で cross-tick 重複排除
3. 同じ `codex_source.ingest()` に流す

### T-119: pane_id correlation (次タスク)

`thread/list` の `cwd` パラメータで tmux pane の cwd とマッチングし、thread ↔ pane 対応を確立する。

### 現実装の既知問題

| Issue | Severity | Status | Description |
|-------|----------|--------|-------------|
| `jsonrpc` field missing | High | **Fixed (T-120)** | 全送信メッセージに `"jsonrpc": "2.0"` が欠落 → B1 で修正。 |
| `thread/list` omits `status` | High | **Fixed** | Real API (v0.104.0) は `status` フィールドを `thread/list` に含めない → `unwrap_or("idle")` で対応。 |
| `used_appserver` flag logic | Medium | **Fixed (T-120)** | App Server alive + 0 events → 不要な capture fallback → B3 で `is_alive()` 判定に修正。 |
| No reconnection | Medium | **Fixed (T-120)** | App Server 終了後の再接続なし → B4 で exponential backoff 再接続追加。 |
| Mutex across await | Low | **Fixed (T-120)** | `poll_threads().await` 中に mutex 保持 → B5 で take/put パターンに修正。 |
| No `params` in `initialized` | Low | **Fixed (T-120)** | `initialized` 通知に `"params": {}` 未設定 → B2 で修正。 |
| `notLoaded` thread pollution | Low | **Fixed** | `notLoaded` ステータスのスレッドが events に含まれる → filter で除外。 |
| Per-cwd query volume | Low | Open | 全スレッドが各 per-cwd query で返される (cwd filter が効いていない可能性) → 1 tick あたり ~50 events。MAX_CWD_QUERIES_PER_TICK=8 で cap。 |
