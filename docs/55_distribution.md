# Distribution Strategy (mutable)

## 決定概要 (2026-02-28)

AIツールが乱立する市場環境では、インストールの容易さは採用の必須条件。
**初日から `brew install` が通る状態を作る**ことを最優先とし、`cargo-dist` で自動化する。

---

## チャネル選定

### Primary: Homebrew tap

```bash
brew install g960059/tap/agtmux
```

- ターゲット（macOS + tmux 常用開発者）の最短導線
- `brew upgrade agtmux` でアップグレード完結
- formula があるだけで「ちゃんとしたツール」というシグナルになる
- 自前 tap から始め、homebrew/core は後で申請（基準: ~75 stars + 30日公開実績）

### Secondary: curl installer (Linux / Homebrew なし macOS)

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/g960059/agtmux/releases/latest/download/install.sh | sh
```

- Linux は `*-unknown-linux-musl` (static binary) で配布 — glibc 依存ゼロ、Alpine 含む全ディストロで動作
- `curl | sh` への不信感は Artifact Attestation + SHA256SUMS で対処

### Tertiary: cargo install (Rust ユーザー)

```bash
cargo install --locked agtmux
```

- ソースビルドのため遅いが、Rust ユーザーには最も信頼されるチャネル
- crates.io publish が前提

---

## 設計上の制約

- **self-update は実装しない**。Homebrew 経由のバイナリが自己更新するとパッケージマネージャーと競合するため
- **`agtmux --version` は tmux なしでも成功する**こと。Homebrew の `test do` ブロックの前提
- **アンインストール手順を README に明記する**。採用の心理的ハードルを下げるため
- **Windows はスコープ外**。tmux が非対応のため、README に明記する

---

## ビルドターゲット

| ターゲット | 対象 |
|-----------|------|
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-apple-darwin` | macOS Intel |
| `x86_64-unknown-linux-musl` | Linux x86_64 |
| `aarch64-unknown-linux-musl` | Linux ARM64 |

musl ターゲットは `cross` クレートで Docker ベースビルドに統一し、再現性を確保する。

---

## CI/CD パイプライン

ツール: **`cargo-dist`** (axodotdev)
- Homebrew tap 自動更新、install.sh 生成、GitHub Actions ワークフロー生成をカバー

### リリースフロー

```
git tag v0.x.x && git push --tags
    │
    ├─ [verify] just verify (fmt + lint + test)
    ├─ [build] cross-compile × 4 targets (cargo-dist)
    ├─ [package] tar.gz + SHA256SUMS + SBOM
    ├─ [attest] GitHub Artifact Attestation (provenance)
    ├─ [release] GitHub Release (draft → publish)
    ├─ [tap] homebrew-tap Formula/agtmux.rb を自動更新 push
    └─ [smoke] クリーン環境で brew install + agtmux --version 検証
```

### Cargo.toml メタデータ (workspace)

```toml
[workspace.metadata.dist]
cargo-dist-version = "0.28"        # 使用時点の最新を指定
ci = ["github"]
installers = ["homebrew", "shell"]
tap = "g960059/homebrew-agtmux"
targets = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
]
pr-run-mode = "plan"
```

### crates.io メタデータ (agtmux-runtime)

```toml
[package]
description = "Real-time AI agent state monitor for tmux"
repository = "https://github.com/g960059/agtmux"
license = "MIT"
keywords = ["tmux", "ai", "agent", "claude", "monitor"]
categories = ["command-line-utilities", "development-tools"]
```

---

## 段階的展開計画

### Phase D-0: インフラ整備（v0.1.0、1週間）

```
□ LICENSE (MIT) を追加
□ Cargo.toml にメタデータ + cargo-dist 設定を追加
□ cargo-dist init でリリースワークフロー生成
□ github.com/g960059/homebrew-agtmux リポジトリ作成
□ HOMEBREW_TAP_TOKEN を GitHub Secrets に設定
□ v0.1.0 タグ push → brew install が通ることを確認
□ README の Install セクションを新チャネルに更新
```

チェックポイント: `brew install g960059/tap/agtmux && agtmux --version` が通ること

### Phase D-1: 発見可能性（v0.1.x〜v0.2.0、1ヶ月）

```
□ crates.io に publish
□ VHS (charmbracelet/vhs) で動作 GIF を生成して README に貼る
□ awesome-tmux に PR
□ Hacker News "Show HN" + r/rust + r/commandline への投稿
```

### Phase D-2: エコシステム統合（v0.3.0〜、3ヶ月）

```
□ Nix flake 追加 (flake.nix をリポジトリに置く)
□ mise プラグイン対応
□ tmux バージョン要件を README に明記 (tmux >= 3.2)
□ homebrew/core への申請検討 (~75 stars + 30日公開実績が目安)
```

### Phase D-3: 長期（v1.0.0〜、6ヶ月以降）

```
□ homebrew/core に正式申請
□ AUR (agtmux-bin) — musl binary を PKGBUILD で配布
□ nixpkgs に PR
```

Windows / winget / scoop はスコープ外（tmux 非対応）と README に明記する。

---

## Homebrew formula テンプレート

`github.com/g960059/homebrew-agtmux/Formula/agtmux.rb`:

```ruby
class Agtmux < Formula
  desc "Real-time AI agent state monitor for tmux"
  homepage "https://github.com/g960059/agtmux"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/g960059/agtmux/releases/download/v#{version}/agtmux-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_ARM"
    end
    on_intel do
      url "https://github.com/g960059/agtmux/releases/download/v#{version}/agtmux-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_X86"
    end
  end

  depends_on "tmux"

  def install
    bin.install "agtmux"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/agtmux --version")
  end
end
```

cargo-dist を使用する場合、このファイルは自動生成・自動更新される。

---

## リスクと対策

| リスク | 対策 |
|--------|------|
| homebrew/core 基準未達 | 自前 tap から始める。焦って申請しない |
| crates.io 名前衝突 | `cargo search agtmux` で事前確認。衝突時は `agtmux-cli` |
| macOS Gatekeeper 警告 | Homebrew 経由を強く推奨。手動バイナリには codesign 手順を補足 |
| musl cross-compile 不安定 | `cross` クレートで Docker ベースビルドに統一 |
| `curl \| sh` への不信 | Artifact Attestation + SHA256SUMS を Release に必ず添付 |
| self-update と Homebrew 競合 | self-update 機能は実装しない |
| AI エージェント仕様変更 | source adapter ごとに独立リリース可能な設計（現行アーキで対応済み） |
