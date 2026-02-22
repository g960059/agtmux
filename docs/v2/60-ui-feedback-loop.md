# AGTMUX v2 UI Feedback Loop Guide

Date: 2026-02-21  
Status: Active  
Depends on: `./10-product-charter.md`, `./40-execution-plan.md`

## 1. Purpose

UI修正のたびに手動確認だけへ戻らないため、最小の自動ループを固定する。

狙い:

1. 起動不能や描画退行を即検知する
2. 連続実行でも壊れないことを確認する
3. 実行ログを markdown artifact で残す

## 2. Non-goals

1. UIテストのみでIME/入力体験を完全保証すること
2. CI上でmacOS TCCまで完全再現すること
3. 画像ピクセル比較で見た目差分を厳密判定すること

## 3. Execution Environment Constraints

1. UIテストは `AGTMUX_RUN_UI_TESTS=1` でのみ有効化する
2. SSHセッション実行は禁止（`SSH_CONNECTION/SSH_TTY` 検出で中断）
3. 実行場所は VM の GUI ログインセッション内 `Terminal.app` または `Xcode`
4. 必須権限:
   1. Accessibility
   2. Screen Recording

## 4. Test Policy

## 4.1 判定レベル

1. `PASS`: 要件を満たす
2. `SKIP`: 環境依存（TCC/AX列挙不安定）で判定不能
3. `FAIL`: 機能退行または起動不能

## 4.2 SKIP運用ルール

1. `window visible` が確認できる場合のみ AX検証の skip を許容
2. `FAIL` を `SKIP` に格下げする時は理由をメッセージに明記する
3. 同じ skip が常態化する場合は、次phaseでテスト基盤改善タスクを切る

## 4.3 検証優先順位

1. window存在（起動・表示）
2. accessibility identifier 検証
3. 補助的に静的テキスト検証（`Sessions` 等）

## 5. Canonical Commands

```bash
# 単発
AGTMUX_RUN_UI_TESTS=1 ./scripts/ui-feedback/run-ui-tests.sh

# 反復
./scripts/ui-feedback/run-ui-loop.sh 3

# 反復 + markdown artifact
./scripts/ui-feedback/run-ui-feedback-report.sh 3
```

主な環境変数:

1. `AGTMUX_RUN_UI_TESTS=1`
2. `AGTMUX_UI_TEST_CAPTURE=1`
3. `AGTMUX_UI_TEST_CAPTURE_DIR=/tmp/agtmux-ui-captures`
4. `AGTMUX_UI_REPORT_PATH=/tmp/agtmux-ui-feedback-report-<ts>.md`
5. `AGTMUX_UI_LOOP_DELAY_SECONDS=2`

## 5.1 Template Bootstrap

このリポジトリにはテンプレートとして次を配置する。

1. `scripts/ui-feedback/run-ui-tests.sh`
2. `scripts/ui-feedback/run-ui-loop.sh`
3. `scripts/ui-feedback/run-ui-feedback-report.sh`

最初にプロジェクトへ合わせる項目:

1. `AGTMUX_UI_TEST_WORKDIR`（例: `macapp`, `apps/desktop`）
2. `AGTMUX_UI_TEST_COMMAND`（例: `swift test --filter AGTMUXDesktopUITests`）

## 6. Required Report Fields

`run-ui-feedback-report.sh` の先頭サマリに必須:

1. `iterations`
2. `status`
3. `runs_completed`
4. `tests_executed`
5. `tests_skipped`
6. `tests_failures`
7. `ui_snapshot_errors`
8. `capture_dir`

## 7. Accessibility Identifier Policy

UIスモーク対象コンポーネントには identifier を付与し、文言変更でテストが壊れないようにする。

最小セット例:

1. `workspace.board`
2. `sidebar.panel`
3. `terminal.panel`
4. `sidebar.header`
5. `sidebar.footer`

## 8. Screenshot Policy

1. スクリーンショット取得は診断補助であり、合否の主判定に使わない
2. `screencapture` 失敗は `ui_snapshot_errors` として集計する
3. 連続失敗時は権限/TCC状態を運用側で再確認する

## 9. Merge Gate (UI)

UIを触るPRでは、最低限次を満たす。

1. `swift test` pass
2. GUIセッションで `scripts/ui-feedback/run-ui-feedback-report.sh 1` 実行
3. `tests_failures = 0`
4. report artifact をPR説明に添付

推奨:

1. 体験に大きく影響する変更は `iterations=3` で実行
2. `tests_skipped` が多い場合は理由をPRに明記

## 10. Manual Review Checklist (Not Automated Yet)

1. IME preedit/候補表示の視認性
2. 日本語確定時のカーソル位置
3. スクロール追従と入力遅延の体感
4. pane切替時の表示崩れ有無
5. animation（working indicator等）のカクつき

## 11. Troubleshooting

1. `UIテストはSSHから実行できません`:
   1. VM GUIログイン側のTerminal/Xcodeで再実行する
2. `権限不足`:
   1. Accessibility/Screen Recording を該当アプリへ付与する
3. `AX検証が毎回skip`:
   1. identifier実装漏れとAXツリー到達性を確認する
4. `ui_snapshot_errors` が高い:
   1. Screen Recording権限と `screencapture` 実行環境を確認する
