# AGTMUX v2 Bootstrap and Workspace Layout

Date: 2026-02-21  
Status: Active  
Depends on: `../20-unified-design.md`, `../40-execution-plan.md`

## 1. Purpose

repo内でゼロから実装を始める時に、ディレクトリ二重化や命名揺れを防ぐ。

## 2. Canonical Root

このリポジトリの root（`/Users/virtualmachine/ghq/github.com/g960059/agtmux`）をそのまま v2 root とする。  
`agtmux-rs` という子ディレクトリは新規作成しない。

## 3. Canonical Layout

```text
agtmux/
  Cargo.toml
  third_party/
    wezterm/              # git submodule (fork repo)
  crates/
    agtmux-protocol/
    agtmux-target/
    agtmux-tmux/
    agtmux-state/
    agtmux-agent-adapters/
    agtmux-store/
    agtmux-daemon/
    agtmux-cli/
  apps/
    desktop-launcher/     # optional wrapper / packaging scripts only
  scripts/
    ui-feedback/
  docs/
    v2/
```

## 4. Bootstrap Commands

```bash
cd /Users/virtualmachine/ghq/github.com/g960059/agtmux

cargo init --vcs none .
mkdir -p crates apps scripts/ui-feedback

cargo new --lib crates/agtmux-protocol
cargo new --lib crates/agtmux-target
cargo new --lib crates/agtmux-tmux
cargo new --lib crates/agtmux-state
cargo new --lib crates/agtmux-agent-adapters
cargo new --lib crates/agtmux-store
cargo new --bin crates/agtmux-daemon
cargo new --bin crates/agtmux-cli
cargo new --bin apps/desktop-launcher

git submodule add <YOUR_WEZTERM_FORK_URL> third_party/wezterm
git -C third_party/wezterm checkout <PINNED_COMMIT_OR_TAG>
```

## 5. Workspace Manifest Policy

`Cargo.toml` (root) で全crateを workspace 管理する。

1. members は上記 canonical layout と一致
2. edition / lints / shared deps は workspace で共通化
3. protocol version は `agtmux-protocol` で一元管理
4. `third_party/wezterm` は workspace members に含めない（submoduleとして独立管理）

## 6. Directory Ownership

1. `crates/agtmux-protocol`: wire schema/codec
2. `crates/agtmux-daemon`: tmux integration + server
3. `third_party/wezterm`: UI host source (`wezterm-gui fork`)
4. `apps/desktop-launcher`: AGTMUX mode起動・配布補助
5. `scripts/ui-feedback`: UI feedback loop scripts

## 7. Migration Note

旧PoC構造（Go + Swift）のファイルは v2実装の正本にしない。  
必要時のみ `docs/v2/references` と比較し、再利用可否を判断する。
