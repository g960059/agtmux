# Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Codex JSON payload parsing | HIGH — 最も複雑な normalization | fixture テストを充実、edge case を網羅 |
| tmux octal escape decoder | HIGH — 壊れると terminal output 全壊 | CJK/emoji corpus test、`Vec<u8>` + `from_utf8_lossy()` |
| Evidence TTL boundary flapping | MEDIUM — TTL 切れ付近で state が振動 | min_stable_duration: 1500ms、UI 側 debounce |
| Hook event 欠落 | MEDIUM — Claude hook がプロセス終了時に送られない | Evidence TTL fallback (90s)、pane_current_command 変化検知 |
| TOML provider 定義の表現力不足 | MEDIUM — 複雑なパターンが TOML で表現できない | adapter struct で escape hatch を用意 |
| daemon 常駐のリソース消費 | LOW — 500ms ポーリング | CPU/メモリ SLO 設定、アイドル時は 2s に緩和 |
| xterm.js macOS IME | MEDIUM — CJK 入力に既知問題 | Phase 5 で対処、CLI MVP には影響なし |
