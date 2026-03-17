# Review Pack — T-XTERM-A5: managed-exit demotion fix

## Objective
- Task: T-XTERM-A5
- Acceptance criteria: confirmed managed Codex pane demotes to unmanaged when `current_cmd` returns to shell

## Summary

Producer-side semantic drift: after a real Codex pane terminates and the pane returns to `current_cmd=zsh`, the daemon kept `presence=managed provider=codex evidence_mode=heuristic` indefinitely.

Root cause confirmed in `crates/agtmux-source-poller/src/detect.rs`:
- `shell_hint = meta.process_hint.as_deref() == Some("shell")` — only triggers when process scan explicitly sets `process_hint="shell"`
- In environments where `scan_all_processes()` fails (`ps: operation not permitted`), `process_hint=None`
- With `process_hint=None` + `current_cmd=zsh` + stale Codex JSON tokens in capture lines, `shell_hint=false`, `capture_match=true` → pane stays in `agent_pane_ids` → step 10c demotion is skipped

## Change scope (2 files)

| ファイル | 変更内容 | 種別 |
|--------|---------|------|
| `crates/agtmux-source-poller/src/detect.rs` | `shell_hint` を `process_hint=Some("shell")` OR `current_cmd ∈ SHELL_CMDS` に拡張 | behavior fix |
| `crates/agtmux-source-poller/src/detect.rs` | `detect_shell_cmd_without_process_hint_suppresses_stale_codex_prompt_tail` テスト追加 | regression test |
| `crates/agtmux-source-poller/src/detect.rs` | `detect_stale_title_not_suppressed_with_capture` → neutral runtime に変更（旧仕様前提を修正） | test fix |
| `crates/agtmux-runtime/src/poll_loop.rs` | `poll_tick_demotes_managed_pane_when_shell_returns_with_stale_codex_capture_lines` テスト追加 | regression test |

## Fix detail

変更前:
```rust
let shell_hint = meta.process_hint.as_deref() == Some("shell");
```

変更後:
```rust
const SHELL_CMDS: &[&str] = &["zsh", "bash", "fish", "sh", "csh", "tcsh", "ksh", "dash", "nu", "pwsh"];
let cmd_lower = meta.current_cmd.to_ascii_lowercase();
let shell_hint = meta.process_hint.as_deref() == Some("shell")
    || SHELL_CMDS.contains(&cmd_lower.as_str());
```

Step 6b の既存 guard（`!has_live_codex_json_signal()` + `tail_has_shell_prompt()`）がそのまま正しく機能する:
- managed-exit: `shell_hint=true` → `has_live_codex_json_signal()=false` (live tokens なし) → return None → demotion ✓
- app-child running: `shell_hint=true` → `has_live_codex_json_signal()=true` + `tail_has_shell_prompt()=false` → return Some (正しく検出継続) ✓

## Important implementation note

FakeTmuxBackend テストでは snapshot 生成時に `current_cmd=zsh` が `process_hint="shell"` に正規化されるため、runtime テストは修正前でも通った。root cause の再現は `detect.rs` の unit test で確認。

## Verification evidence

- `cargo test -p agtmux-source-poller` → PASS
- `cargo test -p agtmux poll_tick_demotes` → PASS (新テスト含む)
- `just verify` → PASS (213 tests)
- `poll_tick_shell_pane_with_prompt_tail_stays_unmanaged` PASS (app-child regression なし)
- `poll_tick_shell_pane_with_live_codex_json_promotes_managed` PASS (app-child live case regression なし)

## Risk declaration
- Breaking change: no
- Behavior change: yes (P0 fix)
  - managed Codex pane が shell に戻った際に demotion が確実に起きるようになる
  - app-child running case（shell wrapper 下で Codex が live streaming 中）は `has_live_codex_json_signal()` guard が保護

## Review Round 1 — NO_GO (P1 Blocking)

Codex review 指摘 (2026-03-11):

**Blocking (P1)**:
- `detect.rs:93-94` — `process_hint=Some("claude"/"codex")` + `current_cmd=zsh` (deep inspection 経由) の場合でも `shell_hint=true` になり、step 6b で Claude が `def.provider != Provider::Codex` で return None → Claude attribution が消える regression
- 原因: `|| SHELL_CMDS.contains(&cmd_lower.as_str())` が explicit agent hint を上書きしていた

## Fix applied (Round 2)

`shell_hint` を `match` に変更して explicit agent hint を最優先:
```rust
let shell_hint = match meta.process_hint.as_deref() {
    Some("claude") | Some("codex") => false,  // explicit agent hint overrides
    Some("shell") => true,
    _ => SHELL_CMDS.contains(&cmd_lower.as_str()),
};
```

追加 test: `detect_shell_cmd_with_explicit_process_hint_stays_managed`

## Review Verdict Round 2 — GO

- P1 blocking 修正済み
- `cargo test -p agtmux-source-poller` → 81 tests PASS
- `just verify` → PASS (213 tests)
- `snapshot_deep_inspection_shell_descendant_*` 既存テスト PASS (explicit agent hint case 保護)
- `detect_shell_cmd_without_process_hint_suppresses_stale_codex_prompt_tail` PASS (root cause fix 保護)
