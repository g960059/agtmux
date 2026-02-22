# Fork Surface Map (v2)

Date: 2026-02-21  
Status: Fixed spec  
Depends on: `../adr/ADR-0001-wezterm-fork-branch-strategy.md`, `../adr/ADR-0004-wezterm-fork-integration-boundary.md`

## 1. Purpose

`wezterm-gui fork` で「どこまで改造してよいか」を固定し、実装中の scope creep を防ぐ。

## 2. Scope of This Spec

この spec の path は **fork repo root** 基準で読む。  
core repo では submodule pointer 更新のみを扱う（ADR-0005）。

## 3. Allowed Change Zones (MVP)

次のパスのみ通常変更を許可する。

1. `wezterm-gui/src/agtmux/**`  
   AGTMUX 専用の domain / view-model / sidebar / menu / layout mutation UI
2. `wezterm-gui/src/integration/**`  
   daemon protocol v3 bridge, stream session, write/resize routing
3. `wezterm-gui/src/gui/hooks/**`  
   wezterm-gui 既存 UI への最小 hook
4. `wezterm-gui/src/bin/wezterm-gui.rs`  
   bootstrap / entry wiring
5. `assets/agtmux/**`  
   app asset / icon / menu text

## 4. Restricted Zones (ADR Required)

次の変更は ADR 承認前に実施しない。

1. `termwiz/**`
2. `wezterm-term/**`
3. `wezterm-mux-server-impl/**`
4. `wezterm-client/**`
5. VT parser / escape handling core

## 5. Integration Boundary

## 5.1 Data ownership

1. session/window/pane topology: daemon (`topology_sync/delta`) が正
2. terminal cell state: `wezterm_term::Terminal` が正
3. UI selection/filter/order: desktop local state が正

## 5.2 Runtime bridge

`AgtmuxRuntimeBridge` の責務を固定する。

1. protocol v3 frame decode/encode
2. pane attach session lifecycle
3. output bytes -> terminal feed
4. input bytes/resize -> daemon write/resize
5. seq/epoch mismatch の fail-closed

bridge は tmux command を直接発行しない。tmux 操作は daemon API 経由のみ。

## 6. Fork Layout (Target)

```text
wezterm-gui/src/
  agtmux/
    domain/
    sidebar/
    menus/
    window_mode/
    layout_mutation/
    state_attention/
  integration/
    runtime_bridge.rs
    session_router.rs
    protocol_v3_client.rs
  gui/
    hooks/
      root_container_hook.rs
      titlebar_hook.rs
      context_menu_hook.rs
```

## 7. CI Enforcement

## 7.1 In core repo (agtmux)

1. `third_party/wezterm` pointer が update window 以外で変更されたら fail
2. pointer変更時は replay regression gate を必須化
3. `scripts/ci/check-submodule-window.sh` で window rule を機械検証する

## 7.2 In fork repo (wezterm-agtmux)

PR で `restricted zones` への変更がある場合:

1. `docs/v2/adr/ADR-xxxx-*.md` が同時に存在すること
2. PR description に rollback plan を含むこと
3. replay regression gate を必須実行すること
4. `scripts/ci/check-fork-surface.sh` で allowed/restricted path を検証する

## 8. Exit Criteria

1. MVP phase C 完了時点で restricted zone 変更が 0（または ADR 付きのみ）
2. fork update window で rebase conflict が許容時間内（<= 1 day）に解消できる
