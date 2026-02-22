# Fork Hook Map Spike (Mandatory Before Phase C)

Date: 2026-02-21  
Status: Active  
Depends on: `../adr/ADR-0004-wezterm-fork-integration-boundary.md`, `../adr/ADR-0005-fork-source-integration-model.md`

## 1. Purpose

`wezterm-gui fork` で最も実装リスクが高い「差し込み点の曖昧さ」を、Phase C 着手前に除去する。

## 2. Required Deliverables

Spike 完了時に次を必須成果物として残す。

1. file-level hook map（fork repo内の実ファイルパス）
2. function-level entry points（呼び出し順序を含む）
3. AGTMUX UI layer の mount point
4. context menu 差し替え点
5. DnD event routing 点
6. input focus / IME event bridge 点
7. metrics tap 点（fps/input/stream）

## 3. Hook Map Template

下記表を埋める（空欄禁止）。

1. `hook_id`
2. `fork_file_path`
3. `function_or_struct`
4. `phase` (`init|render|input|menu|dnd|metrics`)
5. `owner` (`fork_core|agtmux_layer`)
6. `change_type` (`extend|replace|wrap`)
7. `risk` (`low|medium|high`)
8. `test_case_id`

## 4. Acceptance Criteria

1. hook map で `sidebar/menu/dnd/input/metrics` の全機能をカバー
2. 各 hook に対応する UI/integration test case が存在
3. restricted zone を触る hook がある場合は ADR を追加済み
4. Phase C で「hook点調査」を再実施しなくてよい状態になっている

## 5. Storage Location

成果物は次に置く。

1. `docs/v2/implementation-records/phase-a1-fork-hook-map.md`（core repo）
2. `docs/architecture/hook-map.md`（fork repo）

## 6. Gate

Phase C 開始条件:

1. 本 spec の AC を満たす
2. レビューで `hook map sufficient` 判定を取得
