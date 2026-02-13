# AGTMUX 実現可能性・リスク 再レビュー報告

**レビュー日**: 2026-02-13（修正反映後）
**レビュー観点**: 実現可能性・リスク（第2回）

---

## 前回指摘の対応状況

| 前回指摘 | 重要度 | 判定 |
|---|---|---|
| F-01: Claude CLI hook 未検証 | Critical | **Partially Resolved** — contract 定義済みだが実ペイロード未検証 |
| F-02: Codex CLI notify 未検証 | Critical | **Partially Resolved** — 同上 |
| C-01: Phase 0 スコープ過大 | Critical | **Resolved** — スコープ限定・タスク粒度適切化 |
| D-01: CLI バージョン互換 | Major | **Partially Resolved** — tmux >= 3.3 明記、agent CLI バージョンは未記載 |
| P-01: MVP 遅延 | Major | **Resolved** — Phase 分割・exit criteria・Immediate Sprint 明確化 |
| O-01: CI 未設計 | Critical | **Resolved** — TASK-040 + test-catalog Section 7 + plan Section 10 |
| F-03: tmux boot_id | Major | **Resolved** — フォールバック戦略定義 |
| F-04: SSH レイテンシ | Major | **Resolved** — SSH lifecycle 詳細規定 |
| C-02: Action Snapshot 過剰設計 | Major | **Resolved** — Phase 1 で attach のみ、段階的拡張 |
| C-03: Watch 複雑度 | Major | **Partially Resolved** — contract 定義済み、delta 生成の段階化不明確 |
| D-02: 技術スタック未選定 | Major | **Partially Resolved** — SQLite/UDS 決定、実装言語未選定 |
| D-03: tmux バージョン互換 | Minor | **Resolved** — >= 3.3 明記 |
| E-01: daemon クラッシュ回復 | Major | **Resolved** — lifecycle 定義済み |
| E-02: SSH 障害対策 | Major | **Resolved** — target health 遷移ルール定義 |
| E-03: リソース枯渇 | Minor | **Partially Resolved** — retention 定義済み、DB サイズ監視なし |
| P-02: フェーズ不均一 | Minor | **Resolved** |
| P-03: 開発リソース | Info | **Unresolved** |
| O-02: daemon デプロイ | Minor | **Partially Resolved** — UDS/socket 定義済み、自動起動未定義 |
| O-03: デバッグ手段 | Minor | **Partially Resolved** — health endpoint 追加、debug mode 入り方未定義 |

---

## 新規指摘

### [Critical] R-01: 実装言語・フレームワーク未選定
Phase 0 coding 開始のブロッカー。Go / Rust / TypeScript 等の候補評価が即時必要。

### [Major] R-02: Claude/Codex Hook ペイロードの実検証未実施
adapter contract は定義済みだが、実 CLI 出力のキャプチャとフィクスチャ化が未完了。Phase 1 開始時の手戻りリスク。

### [Major] R-03/N-03: CI 環境での sshd harness 実現性
GitHub Actions 等での sshd 起動にはコンテナ/VM レベル権限が必要。早期 PoC が必要。

### [Minor] N-01: UDS socket の macOS 互換性
$XDG_RUNTIME_DIR が macOS でデフォルト未設定。fallback path 定義済みだが、daemon 起動時の socket directory 自動作成の記載が必要。

### [Minor] N-02: Target health 遷移の「consecutive failures」カウント対象不明確
probe のみカウントか、任意コマンド失敗もカウントか。

### [Minor] N-04: Graceful shutdown 時の in-flight action 扱い
実行中の action (SSH コマンド中) への drain timeout が未定義。

### [Minor] M-02: Health endpoint のレスポンススキーマ未定義
TC-040 の「contract is stable」判定に必要。

### [Minor] M-03: Phase Gate Bundle と Nightly テスト
Phase 0 gate に Nightly テスト含む。Phase 0 では CI テストのみ必須とする段階化を検討。

---

## 総合判定

| 重要度 | 件数 |
|---|---|
| Critical | 1 (R-01: 実装言語未選定) |
| Major | 2 (R-02: hook 検証, R-03: CI sshd) |
| Minor | 6 |
| Info | 1 |

**Phase 0 開始ブロッカーは R-01（実装言語）のみ。** これを解決すれば Phase 0 着手可能。R-02/R-03 は Phase 0 中に並行対処可能。
