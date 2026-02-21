# AGTMUX POC Learnings (for v2 reboot)

Date: 2026-02-21  
Status: Stable lessons

## 1. What Worked

1. tmux-first の方向性は有効
2. pane-centric 運用は意思決定速度を上げる
3. adapter-first state 判定は精度向上に効く
4. optimistic UI は create/kill の体感改善に有効

## 2. What Broke Repeatedly

1. snapshot + stream 混在で cursor/scroll が崩壊
2. app側 cursor 推定で CJK/IME ずれが再発
3. local echo と stream echo で二重文字が発生
4. side effects が hot path に混入し fps 低下
5. state 判定を heuristic 依存にすると drift しやすい

## 3. Root Causes

1. terminal state の single source がなかった
2. control/data plane 境界が曖昧だった
3. UI要件と tmux操作要件の結合が弱かった
4. window を補助扱いにし、構造編集導線が不足した

## 4. Rules For v2

1. selected pane stream-only
2. snapshot active path 禁止
3. cursor/IME/scroll 推定禁止
4. mutation_id + lock + rollback 必須
5. adapter source priority を強制

## 5. UX Lessons

1. 並び順は stable を守る
2. filter は `all|managed|attention|pinned` が実用的
3. organize と settings は責務分離
4. session hover と pane hover の密度を統一する
5. window-grouped は optional だが必須機能

## 6. Performance Lessons

1. hot path で subprocess を叩かない
2. selected pane を優先スケジューリングする
3. metrics を先に入れると退行検知が早い
4. replay tests が最も再現性が高い

## 7. Delivery Lessons

1. 方針未固定のまま実装を進めると揺れる
2. 先に invariants と非採用事項を固定すべき
3. 大型変更は gate 定義を先に置く
4. docs を設計単位でまとめると再始動が速い

## 8. UI Feedback Loop Lessons

1. UIテストは `AGTMUX_RUN_UI_TESTS=1` の明示opt-inを必須にする
2. SSHセッション上ではTCC権限が効かないため、GUIログインセッション実行を強制する
3. 権限チェック（Accessibility + Screen Recording）はテスト前に即時fail/skip判定する
4. AX木の列挙は環境依存で揺れるため、`visible window` を確認した上で skip を許容する
5. 文字列一致より `accessibilityIdentifier` の検証を優先する
6. `screencapture` 失敗はノイズとして記録し、テスト本体を落とさない
7. 単発実行だけでなく、loop + markdown report を標準運用にする
8. reportは `runs_completed/tests_executed/tests_skipped/tests_failures/ui_snapshot_errors` を必ず残す
9. UI自動化で代替できない体験（IME候補表示など）は手動確認チェックリストに残す
