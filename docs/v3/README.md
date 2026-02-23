# docs/v3 — AGTMUX CLI-First MVP

## Phase-Based Reading Guide

作業中の Phase に必要なファイルだけ読む。

| Phase | 読むファイル |
|-------|-------------|
| Phase 0: State Engine Foundation | `01-architecture.md`, `02-state-engine.md`, `07-execution-plan.md` §0 |
| Phase 1: tmux Bridge | `03-terminal-backend.md`, `07-execution-plan.md` §1 |
| Phase 2: Daemon + Sources + API | `04-daemon-api.md`, `07-execution-plan.md` §2 |
| Phase 3: CLI Views (MVP) | `05-cli-views.md`, `07-execution-plan.md` §3 |
| Phase 4: Accuracy + Config | `06-quality-gates.md`, `09-test-loop.md` |
| Phase 5: Tauri Desktop | `01-architecture.md`, `07-execution-plan.md` §5 |

テスト設計は常に `09-test-loop.md` を参照。

## File Index

| File | 内容 | 行数目安 |
|------|------|---------|
| `00-product.md` | 解決する問題、ペルソナ、user story、product principles | ~90 |
| `01-architecture.md` | 3-crate 構成、Backend-agnostic core、trait composition、async pipeline | ~250 |
| `02-state-engine.md` | Engine は Evidence のみ受け取る、TOML provider 定義、scoring flow | ~300 |
| `03-terminal-backend.md` | TerminalBackend trait、TmuxBackend: control mode parser、pipe-pane、observer | ~80 |
| `04-daemon-api.md` | JSON-RPC protocol、subscription filter、summary subscription | ~130 |
| `05-cli-views.md` | `agtmux status` / `tui` / `tmux-status` の仕様 | ~80 |
| `06-quality-gates.md` | accuracy gate (dev/beta/release)、performance SLO | ~30 |
| `07-execution-plan.md` | Phase 0–5 の実装計画 | ~150 |
| `08-risks.md` | リスク表 | ~15 |
| `09-test-loop.md` | テスト 3 層戦略 + proptest、fixture 形式、live validation | ~500 |
